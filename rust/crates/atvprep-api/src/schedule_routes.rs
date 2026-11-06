//! Read views and mutations for recurring schedules.
use crate::{
    extract::AuthenticatedSession,
    mutation::{self, MutationError, MutationRequest},
    opaque::Opaque,
    read::{error, timestamp},
    read_jobs::{safe_head, subject},
    routes_auth,
    state::AppState,
};
use atvprep_manager::{
    approval::{ApprovalVerifier, ScheduleParams},
    cron::{CronExpr, Lang},
    limits,
    repo::Ctx,
    schedules::{
        TimeOfDay, days_to_mask, delete_original_setting, effective_cron, mask_to_days,
        next_cron_run_at_system, run_schedule, upcoming_cron_runs_system,
    },
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) async fn mutate(
    state: AppState,
    headers: HeaderMap,
    method: &'static str,
    path: String,
    action: &'static str,
    body: Value,
    work: impl FnOnce(Ctx<'_>, &AppState) -> Result<(u16, Value), MutationError> + Send + 'static,
) -> Response {
    let token =
        routes_auth::cookie(&headers, state.config.session_cookie_secure).unwrap_or_default();
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let origin_allowed = routes_auth::allowed(&headers, &state);
    let error_path = path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| MutationError::Database)?;
        let ctx = Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        };
        let request = MutationRequest {
            token: &token,
            csrf: &csrf,
            origin_allowed,
            requires_open_gate: true,
            method,
            path: &path,
            action,
            idempotency_key: &key,
            body: &body,
            absolute_ttl: i64::try_from(state.config.session_absolute_ttl_seconds).unwrap_or(0),
            idle_ttl: i64::try_from(state.config.session_idle_ttl_seconds).unwrap_or(0),
            csrf_ttl: i64::try_from(state.config.csrf_ttl_seconds).unwrap_or(0),
        };
        mutation::execute(ctx, &request, |tx| work(tx, &state))
    })
    .await;
    match result {
        Ok(Ok((status, body))) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(body),
        )
            .into_response(),
        Ok(Err(error)) => error.response(&error_path),
        Err(_) => MutationError::Database.response(&error_path),
    }
}

fn schedule_id(token: &str, state: &AppState) -> Result<i64, MutationError> {
    Opaque::new(state.api_secret.as_bytes())
        .decode(token, "sc")
        .ok()
        .filter(|id| *id > 0)
        .ok_or(MutationError::NotFound)
}

fn schedule_view(
    conn: &rusqlite::Connection,
    state: &AppState,
    id: i64,
) -> Result<Value, MutationError> {
    let encoded = Opaque::new(state.api_secret.as_bytes()).encode("sc", id);
    let (rows, _) = views(conn, state).map_err(|()| MutationError::Database)?;
    rows.into_iter()
        .find(|value| value["id"].as_str() == Some(&encoded))
        .ok_or(MutationError::NotFound)
}

/// Delete one schedule with single-use CSRF and idempotent replay.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let path = format!("/api/v1/schedules/{token}");
    let body = json!({"schedule_id":token});
    mutate(
        state,
        headers,
        "DELETE",
        path,
        "delete-schedule",
        body,
        move |ctx, state| {
            let id = schedule_id(&token, state)?;
            let count = ctx
                .conn
                .execute("DELETE FROM schedules WHERE id=?1", [id])
                .map_err(|_| MutationError::Database)?;
            if count != 1 {
                return Err(MutationError::NotFound);
            }
            Ok((200, json!({"data":{"deleted":true}})))
        },
    )
    .await
}

/// Run a schedule once. A schedule that changes files queues only while it carries its approval.
pub async fn run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let path = format!("/api/v1/schedules/{token}/run");
    let body = json!({"schedule_id":token});
    mutate(
        state,
        headers,
        "POST",
        path,
        "run-schedule",
        body,
        move |ctx, state| {
            let id = schedule_id(&token, state)?;
            let exists: bool = ctx
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schedules WHERE id=?1)",
                    [id],
                    |row| row.get(0),
                )
                .map_err(|_| MutationError::Database)?;
            if !exists {
                return Err(MutationError::NotFound);
            }
            run_schedule(ctx, id).map_err(|error| run_error(&error))?;
            Ok((200, json!({"data":schedule_view(ctx.conn,state,id)?})))
        },
    )
    .await
}

/// A refusal by the gate (the only conflict a run can meet) is the caller's to retry later: 409
/// with a plain message. Anything else is a server fault.
fn run_error(error: &atvprep_manager::Error) -> MutationError {
    match error {
        atvprep_manager::Error::Conflict(_) => MutationError::Rejected(
            "Cannot run now: the system is in maintenance. Try again later.",
        ),
        _ => MutationError::Database,
    }
}

fn default_max_files() -> i64 {
    25
}

#[derive(Deserialize, Serialize)]
pub struct ScheduleCreate {
    name: String,
    action: String,
    root_id: Option<String>,
    compliance_state: Option<String>,
    /// Simple mode: weekdays (Monday = 0) and a time generate the cron.
    days: Option<Vec<u8>>,
    time: Option<String>,
    /// Takes precedence over `days` and `time`.
    cron: Option<String>,
    /// Defaults to 25 for checking; required (1 to 500) for repairing and converting.
    max_files: Option<i64>,
}

fn valid_days(days: &[u8]) -> bool {
    !days.is_empty() && days.len() <= 7 && days.iter().all(|day| *day <= 6)
}

/// A schedule's timing as stored: the cron and the legacy columns that stand in for it.
struct Timing {
    cron: CronExpr,
    days_mask: u8,
    time: TimeOfDay,
}

impl Timing {
    fn from_cron(cron: CronExpr) -> Self {
        let (days_mask, time) = cron.legacy_columns();
        Self {
            cron,
            days_mask,
            time,
        }
    }

    fn from_weekly(days_mask: u8, time: TimeOfDay) -> Self {
        Self {
            cron: CronExpr::from_weekdays(days_mask, time),
            days_mask,
            time,
        }
    }
}

/// The timing a create request asks for, or `None` when it is missing or invalid.
fn create_timing(payload: &ScheduleCreate) -> Option<Timing> {
    if let Some(text) = payload.cron.as_deref() {
        return CronExpr::parse(text).ok().map(Timing::from_cron);
    }
    let days = payload.days.as_deref().filter(|days| valid_days(days))?;
    let time = TimeOfDay::parse(payload.time.as_deref()?).ok()?;
    Some(Timing::from_weekly(days_to_mask(days), time))
}

#[derive(Deserialize, Serialize)]
pub struct ScheduleUpdate {
    name: Option<String>,
    root_id: Option<String>,
    #[serde(default)]
    clear_root: bool,
    compliance_state: Option<String>,
    #[serde(default)]
    clear_compliance_state: bool,
    days: Option<Vec<u8>>,
    time: Option<String>,
    cron: Option<String>,
    max_files: Option<i64>,
    enabled: Option<bool>,
    /// Required to turn on a schedule that changes files; the confirmation is what signs it.
    #[serde(default)]
    confirm_changes_files: bool,
}

type ExistingSchedule = (
    String,
    Option<i64>,
    Option<String>,
    i64,
    String,
    i64,
    bool,
    String,
    String,
    Option<String>,
);

/// Update the selection or timing of an existing schedule.
pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Json(mut payload): Json<ScheduleUpdate>,
) -> Response {
    let path = format!("/api/v1/schedules/{token}");
    if let Some(name) = payload.name.as_mut() {
        *name = name.trim().to_owned();
    }
    if payload
        .name
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.chars().count() > 80)
        || payload.days.as_ref().is_some_and(|days| !valid_days(days))
        || payload
            .cron
            .as_ref()
            .is_some_and(|cron| CronExpr::parse(cron).is_err())
        || payload
            .time
            .as_ref()
            .is_some_and(|time| TimeOfDay::parse(time).is_err())
        || payload.max_files.is_some_and(|max| max < 1)
        || payload.compliance_state.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "unknown" | "clean" | "warning" | "blocker" | "probe_failed"
            )
        })
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", &path);
    }
    let mut body = serde_json::to_value(&payload).unwrap_or(Value::Null);
    body["schedule_id"] = json!(token);
    let session_token =
        routes_auth::cookie(&headers, state.config.session_cookie_secure).unwrap_or_default();
    mutate(state,headers,"PATCH",path,"update-schedule",body,move |ctx,state|{
        let id=schedule_id(&token,state)?;
        let existing:Option<ExistingSchedule>=ctx.conn.query_row(
            "SELECT name,root_id,compliance_state,days_mask,time_of_day,max_files,enabled,action,armed_at,cron FROM schedules WHERE id=?1",[id],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?))
        ).optional().map_err(|_|MutationError::Database)?;
        let (name,root,compliance,mask,time,max_files,enabled,action,armed,stored_cron)=existing.ok_or(MutationError::NotFound)?;
        let (old_root,old_compliance)=(root,compliance.clone());
        let root=if payload.clear_root {None} else if let Some(value)=payload.root_id.as_deref(){
            let root=Opaque::new(state.api_secret.as_bytes()).decode(value,"r").map_err(|_|MutationError::InvalidInput("root_id must be an opaque identifier"))?;
            let exists:bool=ctx.conn.query_row("SELECT EXISTS(SELECT 1 FROM library_roots WHERE id=?1)",[root],|row|row.get(0)).map_err(|_|MutationError::Database)?;
            if !exists {return Err(MutationError::InvalidInput("unknown library"));}Some(root)
        } else {root};
        let scan=action=="scan";
        let compliance=if scan || payload.clear_compliance_state {None}else{payload.compliance_state.clone().or(compliance)};
        let retimed=payload.cron.is_some() || payload.days.is_some() || payload.time.is_some();
        let rearm=retimed || (payload.enabled==Some(true) && !enabled);
        let (mask,time,cron)=if let Some(text)=payload.cron.as_deref() {
            let timing=Timing::from_cron(CronExpr::parse(text).map_err(|_|MutationError::InvalidInput("invalid cron expression"))?);
            (i64::from(timing.days_mask),timing.time.to_string(),Some(timing.cron.as_str().to_owned()))
        } else if retimed {
            let mask=payload.days.as_ref().map_or(u8::try_from(mask).unwrap_or(127),|days|days_to_mask(days));
            let time=TimeOfDay::parse(payload.time.as_deref().unwrap_or(&time)).map_err(|_|MutationError::InvalidInput("time must be HH:MM"))?;
            let timing=Timing::from_weekly(mask,time);
            (i64::from(mask),time.to_string(),Some(timing.cron.as_str().to_owned()))
        } else {(mask,time,stored_cron)};
        let new_max=if scan {max_files} else {payload.max_files.unwrap_or(max_files)};
        // Limits apply to whatever is saved, so a stored schedule over a limit is rejected here too;
        // pausing it stays possible.
        let pause_only=payload.enabled==Some(false) && !retimed && payload.max_files.is_none();
        if !pause_only {
            let effective=effective_cron(cron.as_deref(),u8::try_from(mask).unwrap_or(127),&time).map_err(|_|MutationError::Database)?;
            limits::check(&action,&effective,new_max).map_err(|violation|MutationError::Validation(violation.to_string()))?;
        }
        let mut new_enabled=payload.enabled.unwrap_or(enabled);
        // A schedule that changes files runs only with the operator's signature on its exact
        // parameters: turning it on (confirmed) signs, a changed parameter or a pause revokes.
        let changes_files=matches!(action.as_str(),"fix"|"convert");
        let mut approval:Option<(i64,i64,String)>=None;
        let mut keep_approval=!changes_files;
        if changes_files {
            let params_changed=root!=old_root || compliance!=old_compliance || new_max!=max_files;
            if payload.enabled==Some(true) {
                if !payload.confirm_changes_files {return Err(MutationError::InvalidInput("turning on a schedule that changes files needs an explicit confirmation"));}
                let approver=crate::routes_jobs::approver(ctx,state,&session_token)?;
                let at=ctx.clock.now_micros()/1_000_000;
                let verifier=ApprovalVerifier::new(state.approval_secret.as_bytes()).ok_or(MutationError::Database)?;
                let delete_original=delete_original_setting(ctx.conn).map_err(|_|MutationError::Database)?;
                let mac=verifier.sign_schedule(&ScheduleParams{schedule_id:id,action:&action,root_id:root,compliance_state:compliance.as_deref(),max_files:new_max,delete_original},approver,at);
                approval=Some((approver,at,mac));
            } else if new_enabled && !params_changed {
                keep_approval=true;
            } else {
                new_enabled=false;
            }
        }
        ctx.conn.execute("UPDATE schedules SET name=?1,root_id=?2,compliance_state=?3,days_mask=?4,time_of_day=?5,max_files=?6,enabled=?7,armed_at=?8,cron=?9 WHERE id=?10",rusqlite::params![payload.name.unwrap_or(name),root,compliance,mask,time,new_max,new_enabled,if rearm {ctx.clock.now()} else {armed},cron,id]).map_err(|_|MutationError::Database)?;
        if !keep_approval {
            let (by,at,mac)=approval.map_or((None,None,None),|(by,at,mac)|(Some(by),Some(at),Some(mac)));
            ctx.conn.execute("UPDATE schedules SET approved_by=?1,approved_at=?2,approval_mac=?3 WHERE id=?4",rusqlite::params![by,at,mac,id]).map_err(|_|MutationError::Database)?;
        }
        Ok((200,json!({"data":schedule_view(ctx.conn,state,id)?})))
    }).await
}

/// Create a non-destructive recurring verify schedule.
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut payload): Json<ScheduleCreate>,
) -> Response {
    const PATH: &str = "/api/v1/schedules";
    payload.name = payload.name.trim().to_owned();
    let Some(timing) = create_timing(&payload) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    };
    if payload.name.is_empty()
        || payload.name.chars().count() > 80
        || payload.max_files.is_some_and(|max| max < 1)
        || (matches!(payload.action.as_str(), "fix" | "convert") && payload.max_files.is_none())
        || payload.compliance_state.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "unknown" | "clean" | "warning" | "blocker" | "probe_failed"
            )
        })
        || !matches!(
            payload.action.as_str(),
            "verify" | "scan" | "fix" | "convert"
        )
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    if let Err(violation) = limits::check(
        &payload.action,
        &timing.cron,
        payload.max_files.unwrap_or_else(default_max_files),
    ) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            &violation.to_string(),
            PATH,
        );
    }
    let token =
        routes_auth::cookie(&headers, state.config.session_cookie_secure).unwrap_or_default();
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let origin_allowed = routes_auth::allowed(&headers, &state);
    let body = serde_json::to_value(&payload).unwrap_or(Value::Null);
    let result=tokio::task::spawn_blocking(move ||{
        let conn=state.db.conn().map_err(|_|MutationError::Database)?;
        let ctx=Ctx{conn:conn.raw(),clock:state.clock.as_ref()};
        let request=MutationRequest{token:&token,csrf:&csrf,origin_allowed,requires_open_gate:true,method:"POST",path:PATH,action:"create-schedule",idempotency_key:&key,body:&body,absolute_ttl:i64::try_from(state.config.session_absolute_ttl_seconds).unwrap_or(0),idle_ttl:i64::try_from(state.config.session_idle_ttl_seconds).unwrap_or(0),csrf_ttl:i64::try_from(state.config.csrf_ttl_seconds).unwrap_or(0)};
        mutation::execute(ctx,&request,|tx|{
            let changes_files=matches!(payload.action.as_str(),"fix"|"convert");
            let scan=payload.action=="scan";
            let count:i64=tx.conn.query_row("SELECT count(*) FROM schedules",[],|row|row.get(0)).map_err(|_|MutationError::Database)?;
            if count>=50{return Err(MutationError::InvalidInput("at most 50 schedules"));}
            let opaque=Opaque::new(state.api_secret.as_bytes());
            let root=if let Some(value)=payload.root_id.as_deref().filter(|value|!value.is_empty()){
                let id=opaque.decode(value,"r").map_err(|_|MutationError::InvalidInput("root_id must be an opaque identifier"))?;
                let exists:bool=tx.conn.query_row("SELECT EXISTS(SELECT 1 FROM library_roots WHERE id=?1)",[id],|row|row.get(0)).map_err(|_|MutationError::Database)?;
                if !exists{return Err(MutationError::InvalidInput("unknown library"));}Some(id)
            }else{None};
            tx.conn.execute("INSERT INTO schedules(name,action,root_id,compliance_state,days_mask,time_of_day,cron,max_files,enabled,armed_at) VALUES(?1,?9,?2,?3,?4,?5,?6,?7,?10,?8)",rusqlite::params![payload.name,root,if scan {None} else {payload.compliance_state.as_deref()},timing.days_mask,timing.time.to_string(),timing.cron.as_str(),if scan {default_max_files()} else {payload.max_files.unwrap_or_else(default_max_files)},tx.clock.now(),payload.action,!changes_files]).map_err(|_|MutationError::Database)?;
            let id=opaque.encode("sc",tx.conn.last_insert_rowid());
            let (rows,_)=views(tx.conn,&state).map_err(|()|MutationError::Database)?;
            let data=rows.into_iter().find(|value|value["id"].as_str()==Some(&id)).ok_or(MutationError::Database)?;
            Ok((201,json!({"data":data})))
        })
    }).await;
    match result {
        Ok(Ok((status, body))) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(body),
        )
            .into_response(),
        Ok(Err(error)) => error.response(PATH),
        Err(_) => MutationError::Database.response(PATH),
    }
}

fn views(conn: &rusqlite::Connection, state: &AppState) -> Result<(Vec<Value>, bool), ()> {
    let opaque = Opaque::new(state.api_secret.as_bytes());
    let now = DateTime::<Utc>::from_timestamp_micros(state.clock.now_micros()).ok_or(())?;
    let mut stmt=conn.prepare("SELECT s.id,s.name,s.action,s.root_id,r.name,s.compliance_state,s.days_mask,s.time_of_day,s.max_files,s.enabled,s.last_run_at,s.last_result,s.last_job_id,s.cron FROM schedules s LEFT JOIN library_roots r ON r.id=s.root_id ORDER BY s.id").map_err(|_|())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, bool>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<i64>>(12)?,
                row.get::<_, Option<String>>(13)?,
            ))
        })
        .map_err(|_| ())?;
    let mut data: Vec<Value> = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;
    for row in rows {
        let (
            id,
            name,
            action,
            root,
            root_name,
            compliance,
            mask,
            clock,
            max_files,
            enabled,
            last_run,
            last_result,
            last_job,
            stored_cron,
        ) = row.map_err(|_| ())?;
        let mask = u8::try_from(mask).map_err(|_| ())?;
        let cron = effective_cron(stored_cron.as_deref(), mask, &clock).map_err(|_| ())?;
        let next = next_cron_run_at_system(now, enabled, &cron).map(|value| value.to_rfc3339());
        let scan = action == "scan";
        let matching: Option<i64> = if scan {
            None
        } else {
            Some(conn.query_row("SELECT count(*) FROM media_files m WHERE m.discovery_state='present' AND (?1 IS NULL OR m.root_id=?1) AND (?2 IS NULL OR m.compliance_state=?2) AND NOT EXISTS(SELECT 1 FROM job_items i WHERE i.media_file_id=m.id AND i.action IN ('fix','convert') AND i.status NOT IN ('succeeded','failed','cancelled','interrupted','stale_input')) AND (CASE WHEN ?3 IN ('fix','convert') THEN NOT EXISTS(SELECT 1 FROM media_files sibling WHERE sibling.root_id=m.root_id AND sibling.discovery_state='present' AND sibling.relative_path=substr(m.relative_path,1,length(m.relative_path)-length(m.suffix))||'.mp4' AND lower(m.suffix) NOT IN ('.mp4','.m4v','.mov')) AND (lower(m.suffix) NOT IN ('.mp4','.m4v','.mov') OR m.compliance_state IN ('warning','blocker') OR m.desired_action='fix') AND (?4 OR m.compliance_reasons NOT LIKE '%] video-reencode:%') ELSE lower(m.suffix) IN ('.mp4','.m4v','.mov') END)",rusqlite::params![root,compliance,action,conn.query_row("SELECT COALESCE((SELECT value_json='true' FROM runtime_settings WHERE key='delete_original'),0)",[],|r|r.get::<_,bool>(0)).map_err(|_|())?],|row|row.get(0)).map_err(|_|())?)
        };
        let last_run = last_run.map(|value| timestamp(&value).replace('Z', "+00:00"));
        let runs = recent_runs(conn, &opaque, id)?;
        let value = json!({"id":opaque.encode("sc",id),"name":safe_head(&name,2048),"action":action,"root_id":root.map(|id|opaque.encode("r",id)),"root_name":root_name.map(|name|subject(&name)),"compliance_state":if scan {None} else {compliance},"cron":cron.as_str(),"days":mask_to_days(mask),"time":clock,"max_files":if scan {None} else {Some(max_files)},"enabled":enabled,"matching":matching,"next_run_at":next,"last_run_at":last_run,"last_result":last_result.map(|value|safe_head(&value,2048)),"last_job_id":last_job.map(|id|opaque.encode("j",id)),"runs":runs});
        let size = serde_json::to_vec(&value).map_err(|_| ())?.len();
        if size > crate::bounds::MAX_ELEMENT_BYTES
            || used.saturating_add(size).saturating_add(data.len() + 128)
                > crate::bounds::MAX_RESPONSE_BYTES
        {
            truncated = true;
            break;
        }
        used += size;
        data.push(value);
    }
    Ok((data, truncated))
}

/// The limits the editor shows, read from the one place that defines them.
fn limits_view() -> Value {
    json!({
        "min_change_interval_minutes": limits::MIN_CHANGE_INTERVAL_MINUTES,
        "max_files": {
            "verify": limits::MAX_FILES_VERIFY,
            "fix": limits::MAX_FILES_FIX,
            "convert": limits::MAX_FILES_CONVERT,
        },
    })
}

/// What a person has typed in the cron field.
#[derive(Deserialize)]
pub struct CronPreviewQuery {
    #[serde(default)]
    expression: String,
    #[serde(default)]
    lang: String,
    /// The action the expression is for; limits of that action apply. Without one, none do.
    #[serde(default)]
    action: String,
}

/// The number of upcoming runs a cron preview lists.
const PREVIEW_RUNS: usize = 5;

/// Describe a cron expression in plain words and list its next runs, or say what is wrong with it.
///
/// A read-only helper for the schedule editor: nothing is stored, so it needs a session but no
/// CSRF proof. An invalid expression is an answer (`valid: false`), not an HTTP error.
pub async fn cron_preview(
    State(state): State<AppState>,
    _session: AuthenticatedSession,
    Query(query): Query<CronPreviewQuery>,
) -> Response {
    let now = DateTime::<Utc>::from_timestamp_micros(state.clock.now_micros());
    let data = match (CronExpr::parse(&query.expression), now) {
        (Ok(cron), Some(_)) if limits::check_timing(&query.action, &cron).is_err() => {
            let violation = limits::check_timing(&query.action, &cron).err();
            json!({"valid": false, "error": violation.map(|v| v.to_string())})
        }
        (Ok(cron), Some(now)) => {
            let runs: Vec<String> = upcoming_cron_runs_system(now, &cron, PREVIEW_RUNS)
                .iter()
                .map(DateTime::to_rfc3339)
                .collect();
            json!({
                "valid": true,
                "expression": cron.as_str(),
                "description": cron.describe(Lang::from_code(&query.lang)),
                "next_runs": runs,
            })
        }
        (Err(reason), _) => {
            let reason = reason.to_string();
            let reason = reason
                .strip_prefix("invalid cron expression: ")
                .unwrap_or(&reason);
            json!({"valid": false, "error": reason})
        }
        (Ok(_), None) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error",
                "/api/v1/schedules/cron-preview",
            );
        }
    };
    Json(json!({"data": data})).into_response()
}

/// The newest runs of one schedule, newest first (maintenance keeps ten).
fn recent_runs(
    conn: &rusqlite::Connection,
    opaque: &Opaque,
    schedule: i64,
) -> Result<Vec<Value>, ()> {
    let mut stmt = conn
        .prepare("SELECT started_at,trigger,result,job_id,scan_id,kind FROM schedule_runs WHERE schedule_id=?1 ORDER BY id DESC LIMIT 10")
        .map_err(|_| ())?;
    let rows = stmt
        .query_map([schedule], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|_| ())?;
    rows.map(|row| {
        let (started, trigger, result, job, scan, kind) = row.map_err(|_| ())?;
        Ok(json!({"kind":kind,"started_at":timestamp(&started).replace('Z', "+00:00"),"trigger":trigger,"result":safe_head(&result,2048),"job_id":job.map(|id|opaque.encode("j",id)),"scan_id":scan.map(|id|opaque.encode("s",id))}))
    })
    .collect()
}

/// List schedules with matching file counts and their next local-calendar run.
pub async fn list(State(state): State<AppState>, _session: AuthenticatedSession) -> Response {
    const PATH: &str = "/api/v1/schedules";
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        let (data, truncated) = views(conn.raw(), &state)?;
        let mut meta = json!({"limits": limits_view()});
        if truncated {
            meta["truncated"] = json!(true);
        }
        Ok::<_, ()>(json!({"data": data, "meta": meta}))
    })
    .await;
    match result {
        Ok(Ok(body)) => Json(body).into_response(),
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
            PATH,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::run_error;
    use atvprep_manager::Error;
    use axum::http::StatusCode;

    #[test]
    fn a_gate_conflict_on_run_now_is_a_409_not_a_500() {
        let conflict = run_error(&Error::Conflict("the deployment gate is closed".to_owned()));
        assert_eq!(
            conflict.response("/api/v1/schedules/x/run").status(),
            StatusCode::CONFLICT
        );
        let broken = run_error(&Error::Invalid("boom".to_owned()));
        assert_eq!(
            broken.response("/api/v1/schedules/x/run").status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}

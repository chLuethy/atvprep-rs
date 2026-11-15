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
use atvprep_core::encode::EncodeAllowance;
use atvprep_manager::{
    approval::{ApprovalVerifier, ScheduleParams},
    cron::{CronExpr, Lang},
    limits, media_sql,
    repo::Ctx,
    schedules::{
        TimeOfDay, days_to_mask, delete_original_setting, effective_cron, mask_to_days,
        next_cron_run_at_system, run_schedule, upcoming_cron_runs_system,
    },
    settings::target_profile,
    state_machine::{ItemStatus, JobAction},
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
    /// Defaults to 25 for checking; required for preparing (limits in [`limits`]).
    max_files: Option<i64>,
    /// Preparing only: the Kodierfreigabe (`copy`, `audio`, `all`). Defaults to `copy`.
    encode: Option<Value>,
}

/// The Kodierfreigabe a request names, `Ok(None)` when it names none, `Err` for anything else.
fn requested_encode(value: Option<&Value>) -> Result<Option<EncodeAllowance>, ()> {
    match value {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .and_then(EncodeAllowance::parse)
            .map(Some)
            .ok_or(()),
    }
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
    /// Preparing only: the Kodierfreigabe; part of what the operator signs when turning it on.
    encode: Option<Value>,
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
    String,
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
        || requested_encode(payload.encode.as_ref()).is_err()
        || payload.compliance_state.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "unknown" | "clean" | "warning" | "blocker" | "probe_failed"
            )
        })
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", &path);
    }
    let requested = requested_encode(payload.encode.as_ref()).unwrap_or(None);
    let mut body = serde_json::to_value(&payload).unwrap_or(Value::Null);
    body["schedule_id"] = json!(token);
    let session_token =
        routes_auth::cookie(&headers, state.config.session_cookie_secure).unwrap_or_default();
    mutate(state,headers,"PATCH",path,"update-schedule",body,move |ctx,state|{
        let id=schedule_id(&token,state)?;
        let existing:Option<ExistingSchedule>=ctx.conn.query_row(
            "SELECT name,root_id,compliance_state,days_mask,time_of_day,max_files,enabled,action,armed_at,cron,encode FROM schedules WHERE id=?1",[id],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?))
        ).optional().map_err(|_|MutationError::Database)?;
        let (name,root,compliance,mask,time,max_files,enabled,action,armed,stored_cron,old_encode)=existing.ok_or(MutationError::NotFound)?;
        let old_encode=EncodeAllowance::parse(&old_encode).unwrap_or(EncodeAllowance::All);
        let (old_root,old_compliance)=(root,compliance.clone());
        let root=if payload.clear_root {None} else if let Some(value)=payload.root_id.as_deref(){
            let root=Opaque::new(state.api_secret.as_bytes()).decode(value,"r").map_err(|_|MutationError::InvalidInput("root_id must be an opaque identifier"))?;
            let exists:bool=ctx.conn.query_row("SELECT EXISTS(SELECT 1 FROM library_roots WHERE id=?1)",[root],|row|row.get(0)).map_err(|_|MutationError::Database)?;
            if !exists {return Err(MutationError::InvalidInput("unknown library"));}Some(root)
        } else {root};
        let scan=action=="scan";
        let changes_files=limits::changes_files(&action);
        if requested.is_some() && !changes_files {return Err(MutationError::InvalidInput("encode applies to preparing only"));}
        let new_encode=requested.unwrap_or(old_encode);
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
        let pause_only=payload.enabled==Some(false) && !retimed && payload.max_files.is_none() && requested.is_none();
        if !pause_only {
            let effective=effective_cron(cron.as_deref(),u8::try_from(mask).unwrap_or(127),&time).map_err(|_|MutationError::Database)?;
            limits::check(&action,new_encode,&effective,new_max).map_err(|violation|MutationError::Validation(violation.to_string()))?;
        }
        let mut new_enabled=payload.enabled.unwrap_or(enabled);
        // A schedule that changes files runs only with the operator's signature on its exact
        // parameters: turning it on (confirmed) signs, a changed parameter or a pause revokes.
        let mut approval:Option<(i64,i64,String)>=None;
        let mut keep_approval=!changes_files;
        if changes_files {
            let params_changed=root!=old_root || compliance!=old_compliance || new_max!=max_files || new_encode!=old_encode;
            if payload.enabled==Some(true) {
                if !payload.confirm_changes_files {return Err(MutationError::InvalidInput("turning on a schedule that changes files needs an explicit confirmation"));}
                let approver=crate::routes_jobs::approver(ctx,state,&session_token)?;
                let at=ctx.clock.now_micros()/1_000_000;
                let verifier=ApprovalVerifier::new(state.approval_secret.as_bytes()).ok_or(MutationError::Database)?;
                let delete_original=delete_original_setting(ctx.conn).map_err(|_|MutationError::Database)?;
                let profile=target_profile(ctx.conn).map_err(|_|MutationError::Database)?;
                let mac=verifier.sign_schedule(&ScheduleParams{schedule_id:id,action:&action,root_id:root,compliance_state:compliance.as_deref(),max_files:new_max,delete_original,encode:new_encode,profile},approver,at);
                approval=Some((approver,at,mac));
            } else if new_enabled && !params_changed {
                keep_approval=true;
            } else {
                new_enabled=false;
            }
        }
        ctx.conn.execute("UPDATE schedules SET name=?1,root_id=?2,compliance_state=?3,days_mask=?4,time_of_day=?5,max_files=?6,enabled=?7,armed_at=?8,cron=?9,encode=?11 WHERE id=?10",rusqlite::params![payload.name.unwrap_or(name),root,compliance,mask,time,new_max,new_enabled,if rearm {ctx.clock.now()} else {armed},cron,id,new_encode.as_str()]).map_err(|_|MutationError::Database)?;
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
    let changes_files = limits::changes_files(&payload.action);
    let Ok(requested) = requested_encode(payload.encode.as_ref()) else {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    };
    if payload.name.is_empty()
        || payload.name.chars().count() > 80
        || payload.max_files.is_some_and(|max| max < 1)
        || (changes_files && payload.max_files.is_none())
        || (requested.is_some() && !changes_files)
        || payload.compliance_state.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "unknown" | "clean" | "warning" | "blocker" | "probe_failed"
            )
        })
        || !matches!(payload.action.as_str(), "verify" | "scan" | "prepare")
    {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    // A new schedule that prepares only copies until the operator chooses more (ADR 0008).
    let encode = requested.unwrap_or(EncodeAllowance::Copy);
    if let Err(violation) = limits::check(
        &payload.action,
        encode,
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
            let scan=payload.action=="scan";
            let count:i64=tx.conn.query_row("SELECT count(*) FROM schedules",[],|row|row.get(0)).map_err(|_|MutationError::Database)?;
            if count>=50{return Err(MutationError::InvalidInput("at most 50 schedules"));}
            let opaque=Opaque::new(state.api_secret.as_bytes());
            let root=if let Some(value)=payload.root_id.as_deref().filter(|value|!value.is_empty()){
                let id=opaque.decode(value,"r").map_err(|_|MutationError::InvalidInput("root_id must be an opaque identifier"))?;
                let exists:bool=tx.conn.query_row("SELECT EXISTS(SELECT 1 FROM library_roots WHERE id=?1)",[id],|row|row.get(0)).map_err(|_|MutationError::Database)?;
                if !exists{return Err(MutationError::InvalidInput("unknown library"));}Some(id)
            }else{None};
            tx.conn.execute("INSERT INTO schedules(name,action,root_id,compliance_state,days_mask,time_of_day,cron,max_files,enabled,armed_at,encode) VALUES(?1,?9,?2,?3,?4,?5,?6,?7,?10,?8,?11)",rusqlite::params![payload.name,root,if scan {None} else {payload.compliance_state.as_deref()},timing.days_mask,timing.time.to_string(),timing.cron.as_str(),if scan {default_max_files()} else {payload.max_files.unwrap_or_else(default_max_files)},tx.clock.now(),payload.action,!changes_files,encode.as_str()]).map_err(|_|MutationError::Database)?;
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
    let mut stmt=conn.prepare("SELECT s.id,s.name,s.action,s.root_id,r.name,s.compliance_state,s.days_mask,s.time_of_day,s.max_files,s.enabled,s.last_run_at,s.last_result,s.last_job_id,s.cron,s.encode FROM schedules s LEFT JOIN library_roots r ON r.id=s.root_id ORDER BY s.id").map_err(|_|())?;
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
                row.get::<_, String>(14)?,
            ))
        })
        .map_err(|_| ())?;
    let embed_open = atvprep_manager::embed::open_media_ids_json(conn).map_err(|_| ())?;
    // What a schedule that prepares is signed against when it is turned on now.
    let profile = target_profile(conn).map_err(|_| ())?.to_json();
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
            encode,
        ) = row.map_err(|_| ())?;
        let mask = u8::try_from(mask).map_err(|_| ())?;
        let cron = effective_cron(stored_cron.as_deref(), mask, &clock).map_err(|_| ())?;
        let next = next_cron_run_at_system(now, enabled, &cron).map(|value| value.to_rfc3339());
        let scan = action == "scan";
        let matching: Option<i64> = if scan {
            None
        } else {
            Some(conn.query_row(&format!("SELECT count(*) FROM media_files m WHERE m.discovery_state='present' AND (?1 IS NULL OR m.root_id=?1) AND (?2 IS NULL OR m.compliance_state=?2) AND NOT EXISTS(SELECT 1 FROM job_items i WHERE i.media_file_id=m.id AND i.action IN {} AND i.status NOT IN {}) AND (CASE WHEN ?3='prepare' THEN {} ELSE {} END)",JobAction::Prepare.stored_sql_list(),ItemStatus::terminal_sql_list(),media_sql::change_candidate("?4","?5"),media_sql::MP4_LIKE),rusqlite::params![root,compliance,action,encode,embed_open],|row|row.get(0)).map_err(|_|())?)
        };
        let last_run = last_run.map(|value| timestamp(&value).replace('Z', "+00:00"));
        let runs = recent_runs(conn, &opaque, id)?;
        let value = json!({"id":opaque.encode("sc",id),"name":safe_head(&name,2048),"action":action,"root_id":root.map(|id|opaque.encode("r",id)),"root_name":root_name.map(|name|subject(&name)),"compliance_state":if scan {None} else {compliance},"cron":cron.as_str(),"days":mask_to_days(mask),"time":clock,"max_files":if scan {None} else {Some(max_files)},"enabled":enabled,"matching":matching,"next_run_at":next,"last_run_at":last_run,"last_result":last_result.map(|value|safe_head(&value,2048)),"last_job_id":last_job.map(|id|opaque.encode("j",id)),"runs":runs});
        let mut value = value;
        if limits::changes_files(&action) {
            value["encode"] = json!(encode);
            value["profile"] = profile.clone();
        }
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
            "prepare": limits::MAX_FILES_PREPARE,
        },
        "max_files_video": limits::MAX_FILES_PREPARE_VIDEO,
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

#[cfg(test)]
mod http_tests {
    use crate::router;
    use crate::routes_auth;
    use crate::test_support::state;
    use atvprep_fixtures::{Fixtures, Verdict};
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::http::header;
    use std::sync::Arc;
    use tower::ServiceExt;

    #[tokio::test]
    async fn schedules_count_only_eligible_unblocked_files() {
        let mut state = state();
        state.clock = Arc::new(atvprep_manager::clock::FixedClock(1_700_000_000_000_000));
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let root = fx.library_at("Movies", "/private");
        let files: Vec<i64> = ["a.mp4", "b.mp4", "c.mkv"]
            .iter()
            .map(|path| {
                fx.file(root, path)
                    .fingerprint(&path[..1])
                    .verdict(Verdict::Clean)
                    .create()
            })
            .collect();
        // b.mp4 is busy with a running Vorbereiten job, so the schedule does not count it.
        let _ = fx.job("fix", &files[1..2]).status("running").create();
        let _ = fx
            .schedule("Weekly", "verify")
            .library(root)
            .days_mask(127)
            .time_of_day("23:59")
            .max_files(25)
            .enabled(true)
            .create();
        let session = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let response = router(state)
            .oneshot(
                axum::http::Request::get("/api/v1/schedules")
                    .header("origin", "http://localhost")
                    .header("cookie", format!("__Host-atvprep_session={session}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["matching"], 1);
        assert_eq!(
            body["data"][0]["days"],
            serde_json::json!([0, 1, 2, 3, 4, 5, 6])
        );
        assert!(body["data"][0]["next_run_at"].is_string());
    }

    #[tokio::test]
    async fn schedule_creation_is_idempotent_and_obeys_the_gate() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let csrf = auth
            .issue_csrf(
                &session.token,
                "POST",
                "/api/v1/schedules",
                "create-schedule",
            )
            .unwrap();
        let app = router(state.clone());
        let body =
            serde_json::json!({"name":"Weekly","action":"verify","days":[0,2],"time":"12:30"});
        let request = |body: &serde_json::Value, key: &str, csrf: &str| {
            axum::http::Request::post("/api/v1/schedules")
                .header("origin", "http://localhost")
                .header(
                    "cookie",
                    format!("__Host-atvprep_session={}", session.token),
                )
                .header("x-csrf-token", csrf)
                .header("idempotency-key", key)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        };
        let first = app
            .clone()
            .oneshot(request(&body, "first", &csrf))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        let first = to_bytes(first.into_body(), 65536).await.unwrap();
        let replay = app
            .clone()
            .oneshot(request(&body, "first", &csrf))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::CREATED);
        assert_eq!(first, to_bytes(replay.into_body(), 65536).await.unwrap());
        let changed =
            serde_json::json!({"name":"Different","action":"verify","days":[0,2],"time":"12:30"});
        assert_eq!(
            app.clone()
                .oneshot(request(&changed, "first", &csrf))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        let count: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM schedules", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let csrf2 = auth
            .issue_csrf(
                &session.token,
                "POST",
                "/api/v1/schedules",
                "create-schedule",
            )
            .unwrap();
        let destructive =
            serde_json::json!({"name":"Unsafe","action":"prepare","days":[0],"time":"12:30"});
        assert_eq!(
            app.clone()
                .oneshot(request(&destructive, "unsafe", &csrf2))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        conn.raw()
            .execute(
                "UPDATE deployment_gate SET mode='maintenance' WHERE id=1",
                [],
            )
            .unwrap();
        let denied = app
            .clone()
            .oneshot(request(&body, "second", &csrf2))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(denied.headers()[header::RETRY_AFTER], "60");
        conn.raw()
            .execute("UPDATE deployment_gate SET mode='open' WHERE id=1", [])
            .unwrap();
        assert_eq!(
            app.oneshot(request(&body, "second", &csrf2))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
    }

    #[tokio::test]
    async fn cron_preview_describes_and_lists_the_next_runs() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let app = router(state.clone());
        let get = |query: &str, authenticated: bool| {
            let mut request = axum::http::Request::builder()
                .uri(format!("/api/v1/schedules/cron-preview{query}"))
                .header("origin", "http://localhost");
            if authenticated {
                request = request.header(
                    "cookie",
                    format!("__Host-atvprep_session={}", session.token),
                );
            }
            let request = request.body(axum::body::Body::empty()).unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let body: serde_json::Value =
                    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                        .unwrap_or(serde_json::Value::Null);
                (status, body)
            }
        };
        let (status, _) = get("?expression=0%203%20*%20*%20*", false).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, body) = get("?expression=0%203%20*%20*%201-5", true).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["valid"], true);
        assert_eq!(body["data"]["expression"], "0 3 * * 1-5");
        assert_eq!(body["data"]["description"], "At 03:00, on weekdays");
        let runs = body["data"]["next_runs"].as_array().unwrap();
        assert_eq!(runs.len(), 5);
        let parsed: Vec<_> = runs
            .iter()
            .map(|run| chrono::DateTime::parse_from_rfc3339(run.as_str().unwrap()).unwrap())
            .collect();
        assert!(parsed.windows(2).all(|pair| pair[0] < pair[1]));

        let (_, body) = get("?expression=*/15%20*%20*%20*%20*&lang=de", true).await;
        assert_eq!(body["data"]["description"], "Alle 15 Minuten");

        let (status, body) = get("?expression=61%20*%20*%20*%20*", true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["valid"], false);
        assert_eq!(body["data"]["error"], "minute: 61 is out of range 0-59");
        assert!(body["data"].get("next_runs").is_none());

        let (status, body) = get("", true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["valid"], false);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One session walks create, list and preview of cron schedules.
    async fn schedules_are_stored_and_returned_as_cron() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let app = router(state.clone());
        let send = |method: &'static str, path: String, body: serde_json::Value, key: &str| {
            let csrf = auth
                .issue_csrf(
                    &session.token,
                    method,
                    &path,
                    if method == "POST" {
                        "create-schedule"
                    } else {
                        "update-schedule"
                    },
                )
                .unwrap();
            let request = axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("origin", "http://localhost")
                .header(
                    "cookie",
                    format!("__Host-atvprep_session={}", session.token),
                )
                .header("x-csrf-token", csrf)
                .header("idempotency-key", key.to_owned())
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let body: serde_json::Value =
                    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                        .unwrap_or(serde_json::Value::Null);
                (status, body)
            }
        };
        // Simple mode: days and time generate the cron (Sunday = 0).
        let (status, body) = send(
            "POST",
            "/api/v1/schedules".into(),
            serde_json::json!({"name":"Simple","action":"verify","days":[0,6],"time":"03:05"}),
            "a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["data"]["cron"], "5 3 * * 0,1");
        assert_eq!(body["data"]["days"], serde_json::json!([0, 6]));
        // A cron is accepted directly and normalised.
        let (status, body) = send(
            "POST",
            "/api/v1/schedules".into(),
            serde_json::json!({"name":"Cron","action":"verify","cron":"*/15  2 * * 7"}),
            "b",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["data"]["cron"], "*/15 2 * * 7");
        let stored: String = conn
            .raw()
            .query_row("SELECT cron FROM schedules WHERE id=2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, "*/15 2 * * 7");
        // Invalid, or neither form given.
        for (key, bad) in [
            (
                "c",
                serde_json::json!({"name":"x","action":"verify","cron":"@daily"}),
            ),
            (
                "d",
                serde_json::json!({"name":"x","action":"verify","cron":"61 * * * *"}),
            ),
            ("e", serde_json::json!({"name":"x","action":"verify"})),
            (
                "f",
                serde_json::json!({"name":"x","action":"verify","days":[1]}),
            ),
        ] {
            let (status, _) = send("POST", "/api/v1/schedules".into(), bad, key).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{key}");
        }
        // Update by cron, then by days (which regenerates the cron).
        let id = crate::opaque::Opaque::new(state.api_secret.as_bytes()).encode("sc", 1);
        let path = format!("/api/v1/schedules/{id}");
        let (status, body) = send(
            "PATCH",
            path.clone(),
            serde_json::json!({"cron":"0 4 1 * *"}),
            "g",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["cron"], "0 4 1 * *");
        let (status, _) = send(
            "PATCH",
            path.clone(),
            serde_json::json!({"cron":"nope"}),
            "h",
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, body) = send("PATCH", path, serde_json::json!({"days":[2]}), "i").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["cron"], "0 4 * * 3");
    }

    #[tokio::test]
    async fn a_missed_slot_is_listed_in_the_history_as_missed() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let _ = fx.schedule("Nightly", "verify").enabled(true).create();
        // The run history has no repository to write it.
        fx.sql("INSERT INTO schedule_runs(schedule_id,slot,started_at,trigger,result,kind) VALUES(1,'2026-11-05 12:30:00.000000','2026-11-05 20:00:00.000000','scheduled','Missed: not started within 360 minutes of its slot','missed');");
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let request = axum::http::Request::builder()
            .uri("/api/v1/schedules")
            .header("origin", "http://localhost")
            .header(
                "cookie",
                format!("__Host-atvprep_session={}", session.token),
            )
            .body(axum::body::Body::empty())
            .unwrap();
        let response = router(state.clone()).oneshot(request).await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        let runs = body["data"][0]["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["kind"], "missed");
        assert_eq!(runs[0]["trigger"], "scheduled");
        assert_eq!(
            runs[0]["result"],
            "Missed: not started within 360 minutes of its slot"
        );
        assert!(runs[0]["job_id"].is_null());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One session walks every limit through create, update, preview and list.
    async fn schedule_limits_are_enforced_on_save_and_published() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let _ = Fixtures::new(conn.raw())
            .schedule("Old prepare", "prepare")
            .max_files(20)
            .enabled(false)
            .create();
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let app = router(state.clone());
        let send = |method: &'static str, path: String, body: serde_json::Value, key: &str| {
            let csrf = auth
                .issue_csrf(
                    &session.token,
                    method,
                    &path,
                    if method == "POST" {
                        "create-schedule"
                    } else {
                        "update-schedule"
                    },
                )
                .unwrap();
            let request = axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("origin", "http://localhost")
                .header(
                    "cookie",
                    format!("__Host-atvprep_session={}", session.token),
                )
                .header("x-csrf-token", csrf)
                .header("idempotency-key", key.to_owned())
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let body: serde_json::Value =
                    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                        .unwrap_or(serde_json::Value::Null);
                (status, body)
            }
        };
        let create = |name: &str, action: &str, cron: &str, max: i64, key: &str| {
            send(
                "POST",
                "/api/v1/schedules".into(),
                serde_json::json!({"name":name,"action":action,"cron":cron,"max_files":max}),
                key,
            )
        };
        // Too frequent for a schedule that changes files, with the minimum named.
        for (key, cron) in [("a", "0,10 * * * *"), ("b", "* * * * *")] {
            let (status, body) = create("x", "prepare", cron, 5, key).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{cron}");
            assert!(body["detail"].as_str().unwrap().contains("15"), "{body}");
        }
        // Too many files, with the limit named.
        for (key, action, max, limit) in [("c", "prepare", 13, "12"), ("e", "verify", 501, "500")] {
            let (status, body) = create("x", action, "*/15 * * * *", max, key).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{action}");
            assert!(body["detail"].as_str().unwrap().contains(limit), "{body}");
        }
        // At the limits, and the actions without one.
        for (key, action, cron, max) in [
            ("f", "prepare", "*/15 * * * *", 12),
            ("g", "prepare", "0,30 * * * *", 6),
            ("h", "verify", "* * * * *", 500),
            ("i", "scan", "* * * * *", 25),
        ] {
            let (status, body) = create(action, action, cron, max, key).await;
            assert_eq!(status, StatusCode::CREATED, "{action}: {body}");
        }
        // An old schedule over a limit is rejected on its next save, but may still be paused.
        let opaque = crate::opaque::Opaque::new(state.api_secret.as_bytes());
        let old = format!("/api/v1/schedules/{}", opaque.encode("sc", 1));
        let (status, body) = send(
            "PATCH",
            old.clone(),
            serde_json::json!({"name":"Renamed"}),
            "j",
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body["detail"].as_str().unwrap().contains("12"), "{body}");
        let (status, _) = send(
            "PATCH",
            old.clone(),
            serde_json::json!({"cron":"* * * * *","max_files":12}),
            "l",
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, _) = send(
            "PATCH",
            old.clone(),
            serde_json::json!({"enabled":false}),
            "n",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "pausing is always allowed");
        let (status, _) = send(
            "PATCH",
            old,
            serde_json::json!({"max_files":12,"name":"Renamed"}),
            "k",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // The preview takes the action; the list publishes the limits.
        let get = |uri: String| {
            let request = axum::http::Request::builder()
                .uri(uri)
                .header("origin", "http://localhost")
                .header(
                    "cookie",
                    format!("__Host-atvprep_session={}", session.token),
                )
                .body(axum::body::Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                serde_json::from_slice::<serde_json::Value>(
                    &to_bytes(response.into_body(), 65536).await.unwrap(),
                )
                .unwrap()
            }
        };
        let fix = get(
            "/api/v1/schedules/cron-preview?expression=0,10%20*%20*%20*%20*&action=prepare".into(),
        )
        .await;
        assert_eq!(fix["data"]["valid"], false);
        assert!(
            fix["data"]["error"].as_str().unwrap().contains("15"),
            "{fix}"
        );
        let verify = get(
            "/api/v1/schedules/cron-preview?expression=0,10%20*%20*%20*%20*&action=verify".into(),
        )
        .await;
        assert_eq!(verify["data"]["valid"], true);
        let none =
            get("/api/v1/schedules/cron-preview?expression=0,10%20*%20*%20*%20*".into()).await;
        assert_eq!(none["data"]["valid"], true);
        let listed = get("/api/v1/schedules".into()).await;
        assert_eq!(
            listed["meta"]["limits"],
            serde_json::json!({"min_change_interval_minutes":15,"max_files":{"verify":500,"prepare":12},"max_files_video":6})
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One walk through run, replay, legacy run and delete.
    async fn schedule_run_and_delete_are_idempotent_and_an_unapproved_changing_run_queues_nothing()
    {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let root = fx.library_at("Movies", "/private");
        let _ = fx
            .file(root, "film.mp4")
            .fingerprint("a")
            .verdict(Verdict::Clean)
            .create();
        let _ = fx.schedule("Verify", "verify").enabled(true).create();
        let _ = fx.schedule("Legacy", "prepare").enabled(false).create();
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let opaque = crate::opaque::Opaque::new(state.api_secret.as_bytes());
        let schedule = opaque.encode("sc", 1);
        let legacy = opaque.encode("sc", 2);
        let run_path = format!("/api/v1/schedules/{schedule}/run");
        let delete_path = format!("/api/v1/schedules/{schedule}");
        let legacy_path = format!("/api/v1/schedules/{legacy}/run");
        let run_csrf = auth
            .issue_csrf(&session.token, "POST", &run_path, "run-schedule")
            .unwrap();
        let delete_csrf = auth
            .issue_csrf(&session.token, "DELETE", &delete_path, "delete-schedule")
            .unwrap();
        let legacy_csrf = auth
            .issue_csrf(&session.token, "POST", &legacy_path, "run-schedule")
            .unwrap();
        let app = router(state.clone());
        let request = |method: axum::http::Method, path: &str, csrf: &str, key: &str| {
            axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("origin", "http://localhost")
                .header(
                    "cookie",
                    format!("__Host-atvprep_session={}", session.token),
                )
                .header("x-csrf-token", csrf)
                .header("idempotency-key", key)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let run = || request(axum::http::Method::POST, &run_path, &run_csrf, "run");
        let first = app.clone().oneshot(run()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first = to_bytes(first.into_body(), 65536).await.unwrap();
        let shown: serde_json::Value = serde_json::from_slice(&first).unwrap();
        let runs = shown["data"]["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["trigger"], "manual");
        assert_eq!(runs[0]["kind"], "run");
        assert_eq!(runs[0]["result"], "Queued 1 file");
        assert!(runs[0]["job_id"].as_str().unwrap().starts_with("j_"));
        let replay = app.clone().oneshot(run()).await.unwrap();
        assert_eq!(first, to_bytes(replay.into_body(), 65536).await.unwrap());
        assert_eq!(
            app.clone()
                .oneshot(request(
                    axum::http::Method::POST,
                    &legacy_path,
                    &legacy_csrf,
                    "legacy"
                ))
                .await
                .unwrap()
                .status(),
            // An unapproved schedule that changes files runs "successfully" but queues nothing.
            StatusCode::OK
        );
        let delete = || {
            request(
                axum::http::Method::DELETE,
                &delete_path,
                &delete_csrf,
                "delete",
            )
        };
        assert_eq!(
            app.clone().oneshot(delete()).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            app.oneshot(delete()).await.unwrap().status(),
            StatusCode::OK
        );
        // Deleting removes the history; only the legacy schedule's "Not queued" run is left.
        let mut stmt = conn
            .raw()
            .prepare("SELECT result FROM schedule_runs")
            .unwrap();
        let history: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(history.len() == 1 && history[0].starts_with("Not queued: "));
        let jobs: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM jobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(jobs, 1);
        let action: String = conn
            .raw()
            .query_row("SELECT action FROM jobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(action, "verify");
        let schedules: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM schedules", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schedules, 1);
    }

    #[tokio::test]
    async fn schedule_updates_rearm_and_need_confirmation_to_turn_on_a_changing_action() {
        let mut state = state();
        state.clock = Arc::new(atvprep_manager::clock::FixedClock(1_700_000_000_000_000));
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let root = fx.library_at("Movies", "/private");
        let _ = fx
            .schedule("Old", "verify")
            .library(root)
            .compliance_state("clean")
            .enabled(false)
            .armed_at("2020-01-01 00:00:00")
            .create();
        let _ = fx
            .schedule("Legacy", "prepare")
            .enabled(false)
            .armed_at("2020-01-01 00:00:00")
            .create();
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let opaque = crate::opaque::Opaque::new(state.api_secret.as_bytes());
        let path = format!("/api/v1/schedules/{}", opaque.encode("sc", 1));
        let legacy = format!("/api/v1/schedules/{}", opaque.encode("sc", 2));
        let csrf = auth
            .issue_csrf(&session.token, "PATCH", &path, "update-schedule")
            .unwrap();
        let legacy_csrf = auth
            .issue_csrf(&session.token, "PATCH", &legacy, "update-schedule")
            .unwrap();
        let app = router(state.clone());
        let request = |path: &str, csrf: &str, key: &str, body: serde_json::Value| {
            axum::http::Request::patch(path)
                .header("origin", "http://localhost")
                .header(
                    "cookie",
                    format!("__Host-atvprep_session={}", session.token),
                )
                .header("x-csrf-token", csrf)
                .header("idempotency-key", key)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        };
        let body = serde_json::json!({"name":" Changed ","days":[1,4],"enabled":true,"clear_root":true,"clear_compliance_state":true});
        let response = app
            .clone()
            .oneshot(request(&path, &csrf, "update", body.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = to_bytes(response.into_body(), 65536).await.unwrap();
        let replay = app
            .clone()
            .oneshot(request(&path, &csrf, "update", body))
            .await
            .unwrap();
        assert_eq!(first, to_bytes(replay.into_body(), 65536).await.unwrap());
        let row:(String,Option<i64>,Option<String>,i64,bool,String)=conn.raw().query_row("SELECT name,root_id,compliance_state,days_mask,enabled,armed_at FROM schedules WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
        assert_eq!(
            row,
            (
                "Changed".to_owned(),
                None,
                None,
                18,
                true,
                atvprep_manager::clock::format_micros(1_700_000_000_000_000)
            )
        );
        let response = app
            .oneshot(request(
                &legacy,
                &legacy_csrf,
                "legacy",
                serde_json::json!({"enabled":true}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let enabled: bool = conn
            .raw()
            .query_row("SELECT enabled FROM schedules WHERE id=2", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!enabled);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One session walks create, run, update and list of a scan schedule.
    async fn scan_schedules_are_created_listed_and_run_without_file_filters() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let fx = Fixtures::new(conn.raw());
        let _ = fx.library_at("Movies", "/private");
        let _ = fx.library_at("Shows", "/shows");
        let _ = fx.schedule("Old repair", "prepare").enabled(false).create();
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let opaque = crate::opaque::Opaque::new(state.api_secret.as_bytes());
        let app = router(state.clone());
        let send = |method: axum::http::Method,
                    path: String,
                    action: &'static str,
                    key: &'static str,
                    body: serde_json::Value| {
            let csrf = auth
                .issue_csrf(&session.token, method.as_str(), &path, action)
                .unwrap();
            let app = app.clone();
            let cookie = format!("__Host-atvprep_session={}", session.token);
            async move {
                let response = app
                    .oneshot(
                        axum::http::Request::builder()
                            .method(method)
                            .uri(path)
                            .header("origin", "http://localhost")
                            .header("cookie", cookie)
                            .header("x-csrf-token", csrf)
                            .header("idempotency-key", key)
                            .header("content-type", "application/json")
                            .body(axum::body::Body::from(body.to_string()))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = response.status();
                let json: serde_json::Value =
                    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                        .unwrap_or(serde_json::Value::Null);
                (status, json)
            }
        };
        let root = opaque.encode("r", 2);
        let (status, created) = send(
            axum::http::Method::POST,
            "/api/v1/schedules".to_owned(),
            "create-schedule",
            "scan-create",
            serde_json::json!({"name":"Nightly scan","action":"scan","root_id":root,"compliance_state":"clean","days":[0],"time":"02:00","max_files":9}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["data"]["action"], "scan");
        assert_eq!(created["data"]["root_name"], "Shows");
        assert!(created["data"]["compliance_state"].is_null());
        assert!(created["data"]["max_files"].is_null());
        assert!(created["data"]["matching"].is_null());
        assert!(created["data"]["next_run_at"].is_string());
        let id = created["data"]["id"].as_str().unwrap().to_owned();
        let (status, _) = send(
            axum::http::Method::POST,
            "/api/v1/schedules".to_owned(),
            "create-schedule",
            "prepare-create",
            serde_json::json!({"name":"Old word","action":"fix","days":[0],"time":"02:00","max_files":5}),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, ran) = send(
            axum::http::Method::POST,
            format!("/api/v1/schedules/{id}/run"),
            "run-schedule",
            "scan-run",
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ran["data"]["last_result"], "Queued 1 scan");
        let queued: (i64, String) = conn
            .raw()
            .query_row(
                "SELECT root_id, status FROM scan_runs ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(queued, (2, "queued".to_owned()));
        let (status, patched) = send(
            axum::http::Method::PATCH,
            format!("/api/v1/schedules/{id}"),
            "update-schedule",
            "scan-patch",
            serde_json::json!({"max_files":3,"compliance_state":"clean","clear_root":true}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(patched["data"]["max_files"].is_null());
        assert!(patched["data"]["compliance_state"].is_null());
        assert!(patched["data"]["root_id"].is_null());
        let listed = app
            .oneshot(
                axum::http::Request::get("/api/v1/schedules")
                    .header("origin", "http://localhost")
                    .header(
                        "cookie",
                        format!("__Host-atvprep_session={}", session.token),
                    )
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed: serde_json::Value =
            serde_json::from_slice(&to_bytes(listed.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(listed["data"][0]["action"], "prepare");

        assert_eq!(listed["data"][1]["action"], "scan");
    }
}

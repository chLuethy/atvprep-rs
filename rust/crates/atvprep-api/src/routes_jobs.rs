//! Mutations that remain available during maintenance.
use crate::{
    mutation::{self, MutationError, MutationRequest},
    opaque::Opaque,
    routes_auth,
    schedule_routes::mutate,
    state::AppState,
};
use atvprep_manager::{
    jobs::{Jobs, NewJob},
    media::Media,
    repo::Ctx,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, Serialize)]
pub struct CreateJob {
    action: String,
    media_ids: Vec<String>,
    #[serde(default)]
    manual_approval: bool,
    #[serde(default)]
    confirm_delete_original: bool,
}

/// Queue inspection or verification through the authenticated mutation pipeline.
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateJob>,
) -> Response {
    const PATH: &str = "/api/v1/jobs";
    if input.media_ids.is_empty()
        || input.media_ids.len() > 100
        || !matches!(
            input.action.as_str(),
            "inspect" | "verify" | "prepare" | "fix" | "convert"
        )
    {
        return crate::read::error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let body = serde_json::to_value(&input).expect("request serializes");
    let session_token =
        routes_auth::cookie(&headers, state.config.session_cookie_secure).unwrap_or_default();
    mutate(state,headers,"POST",PATH.to_owned(),"create-job",body,move |ctx,state| {
        let prepares=matches!(input.action.as_str(),"prepare"|"fix"|"convert");
        let opaque=Opaque::new(state.api_secret.as_bytes());
        let ids=input.media_ids.iter().map(|value|opaque.decode(value,"m").ok().filter(|id|*id>0).ok_or(MutationError::InvalidInput("media_ids must contain opaque identifiers"))).collect::<Result<Vec<_>,_>>()?;
        if prepares && input.confirm_delete_original {return Err(MutationError::InvalidInput("deleting an original is not available"));}
        if prepares && !input.manual_approval {return Err(MutationError::InvalidInput("an explicit approval is required to change files"));}
        for id in &ids {let media=Media(ctx).get(*id).map_err(|_|MutationError::Database)?.ok_or(MutationError::NotFound)?;if prepares && media.discovery_state!="present" {return Err(MutationError::InvalidInput("only files that are present can be prepared"));}}
        let mut new=NewJob::new(if prepares {"fix"}else{&input.action},&ids);new.trigger="manual";
        new.settings_snapshot=if prepares {let approver=approver(ctx,state,&session_token)?;json!({"pipeline":"prepare","policy":"copy-only-v1","delete_original":false,"delete_original_confirmed":false,"manual_approval":true,"approved_by":approver,"approved_at":ctx.clock.now_micros()/1_000_000})}else{json!({"manual_approval":input.manual_approval})};
        let job=Jobs(ctx).create(&new).map_err(|error|match error{atvprep_manager::Error::Invalid(message)=>MutationError::Validation(message),_=>MutationError::Database})?;
        if prepares {sign_job(ctx,state,job.id)?;}
        Ok((201,json!({"data":{"id":opaque.encode("j",job.id),"action":if prepares {"prepare"}else{&job.action},"status":job.status}})))
    }).await
}

pub(crate) fn approver(ctx: Ctx<'_>, state: &AppState, token: &str) -> Result<i64, MutationError> {
    crate::auth::AuthService::with_csrf_ttl(
        ctx,
        i64::try_from(state.config.session_absolute_ttl_seconds).unwrap_or(0),
        i64::try_from(state.config.session_idle_ttl_seconds).unwrap_or(0),
        i64::try_from(state.config.csrf_ttl_seconds).unwrap_or(0),
    )
    .map_err(|_| MutationError::Database)?
    .authenticate(token)
    .map_err(|_| MutationError::Unauthorized)
}

fn sign_job(ctx: Ctx<'_>, state: &AppState, job: i64) -> Result<(), MutationError> {
    use atvprep_manager::{
        approval::{ApprovalItem, ApprovalVerifier},
        snapshots::InputSnapshots,
    };
    let verifier =
        ApprovalVerifier::new(state.approval_secret.as_bytes()).ok_or(MutationError::Database)?;
    for item in Jobs(ctx).items(job).map_err(|_| MutationError::Database)? {
        let Some(recorded) = InputSnapshots(ctx)
            .get_for_item(item.id)
            .map_err(|_| MutationError::Database)?
        else {
            continue;
        };
        let identity = [
            recorded["file_device"].clone(),
            recorded["file_inode"].clone(),
            recorded["file_ctime_ns"].clone(),
            recorded["sidecar_fingerprint"].clone(),
        ];
        let mut snapshot = item.settings_snapshot;
        let person = snapshot["approved_by"]
            .as_i64()
            .ok_or(MutationError::Database)?;
        let time = snapshot["approved_at"]
            .as_i64()
            .ok_or(MutationError::Database)?;
        snapshot["approval_mac"] = json!(verifier.sign(
            &ApprovalItem {
                job_id: job,
                media_id: item.media_file_id,
                action: &item.action,
                identity: &identity
            },
            person,
            time
        ));
        ctx.conn
            .execute(
                "UPDATE job_items SET settings_snapshot_json=?1 WHERE id=?2",
                rusqlite::params![snapshot.to_string(), item.id],
            )
            .map_err(|_| MutationError::Database)?;
    }
    Ok(())
}

/// Retry failed items with a fresh approval bound to this session and new job.
pub async fn retry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let path = format!("/api/v1/jobs/{token}/retry");
    let body = json!({"job_id":token,"operation":"retry"});
    let session =
        routes_auth::cookie(&headers, state.config.session_cookie_secure).unwrap_or_default();
    mutate(
        state,
        headers,
        "POST",
        path,
        "retry-job",
        body,
        move |ctx, state| {
            let opaque = Opaque::new(state.api_secret.as_bytes());
            let id = opaque.decode(&token, "j").ok().filter(|id| *id > 0).ok_or(
                MutationError::InvalidInput("job_id must be an opaque identifier"),
            )?;
            if Jobs(ctx)
                .get(id)
                .map_err(|_| MutationError::Database)?
                .is_none()
            {
                return Err(MutationError::NotFound);
            }
            let person = approver(ctx, state, &session)?;
            let job = Jobs(ctx)
                .retry(id, Some(person))
                .map_err(|error| match error {
                    atvprep_manager::Error::Invalid(message) => match message.as_str() {
                        "job has no retryable failed items" => {
                            MutationError::Rejected("job has no retryable failed items")
                        }
                        "active job item already exists for media" => {
                            MutationError::Rejected("active job item already exists for media")
                        }
                        "a file changed since the job was queued; queue it again" => {
                            MutationError::Rejected(
                                "a file changed since the job was queued; queue it again",
                            )
                        }
                        _ => MutationError::Rejected("job cannot be retried"),
                    },
                    _ => MutationError::Database,
                })?;
            if job.settings_snapshot["pipeline"] == "prepare" {
                sign_job(ctx, state, job.id)?;
            }
            Ok((
                200,
                json!({"data":{"id":opaque.encode("j",job.id),"status":job.status}}),
            ))
        },
    )
    .await
}

/// Request cancellation even while maintenance blocks new work.
pub async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let path = format!("/api/v1/jobs/{token}/cancel");
    let body = json!({"job_id":token,"operation":"cancel"});
    let opaque = Opaque::new(state.api_secret.as_bytes());
    maintenance_mutation(state, headers, path, "cancel-job", body, move |ctx| {
        let id = opaque
            .decode(&token, "j")
            .ok()
            .filter(|id| *id > 0)
            .ok_or(MutationError::NotFound)?;
        let job = Jobs(ctx)
            .get(id)
            .map_err(|_| MutationError::Database)?
            .ok_or(MutationError::NotFound)?;
        let status = if matches!(job.status.as_str(), "queued" | "running") {
            atvprep_manager::state_machine::JobStateMachine(ctx)
                .transition_job(
                    id,
                    "cancel_requested",
                    job.status_version,
                    Some(&json!({"reason":"user_requested"})),
                )
                .map_err(|_| MutationError::Rejected("job changed concurrently"))?;
            ctx.conn
                .execute(
                    "UPDATE jobs SET cancel_requested_at=?1 WHERE id=?2",
                    rusqlite::params![ctx.clock.now(), id],
                )
                .map_err(|_| MutationError::Database)?;
            "cancel_requested".to_owned()
        } else {
            job.status
        };
        Ok((200, json!({"data":{"id":token,"status":status}})))
    })
    .await
}

/// Queue a single non-destructive inspection job.
pub async fn inspect_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    let path = format!("/api/v1/files/{token}/inspect");
    let body = json!({"file_id":token});
    mutate(
        state,
        headers,
        "POST",
        path,
        "inspect",
        body,
        move |ctx, state| {
            let opaque = Opaque::new(state.api_secret.as_bytes());
            let media = opaque
                .decode(&token, "m")
                .ok()
                .filter(|id| *id > 0)
                .ok_or(MutationError::NotFound)?;
            if Media(ctx)
                .get(media)
                .map_err(|_| MutationError::Database)?
                .is_none()
            {
                return Err(MutationError::NotFound);
            }
            let ids = [media];
            let mut new = NewJob::new("inspect", &ids);
            new.trigger = "manual";
            let job = Jobs(ctx)
                .create(&new)
                .map_err(|_| MutationError::Database)?;
            Ok((
                201,
                json!({"data":{"id":opaque.encode("j",job.id),"status":job.status}}),
            ))
        },
    )
    .await
}

/// Hide missing files without deleting their history.
pub async fn dismiss_missing(State(state): State<AppState>, headers: HeaderMap) -> Response {
    maintenance_mutation(
        state,
        headers,
        "/api/v1/files/dismiss-missing",
        "dismiss-missing",
        json!({}),
        |tx| {
            let removed = Media(tx)
                .dismiss_missing()
                .map_err(|_| MutationError::Database)?;
            Ok((200, json!({"data":{"removed":removed}})))
        },
    )
    .await
}

#[derive(Deserialize)]
pub struct ClearJobs {
    statuses: Vec<String>,
}

/// Hide selected finished jobs without deleting their items or events.
pub async fn clear(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ClearJobs>,
) -> Response {
    const PATH: &str = "/api/v1/jobs/clear";
    if input.statuses.is_empty()
        || input.statuses.len() > 4
        || input.statuses.iter().any(|status| {
            !matches!(
                status.as_str(),
                "succeeded" | "partial_success" | "failed" | "cancelled"
            )
        })
    {
        return crate::read::error(StatusCode::UNPROCESSABLE_ENTITY, "invalid request", PATH);
    }
    let body = json!({"statuses":input.statuses});
    maintenance_mutation(state, headers, PATH, "clear-jobs", body, move |tx| {
        let statuses: Vec<&str> = input.statuses.iter().map(String::as_str).collect();
        let cleared = Jobs(tx)
            .clear_finished(&statuses)
            .map_err(|_| MutationError::Database)?;
        Ok((200, json!({"data":{"cleared":cleared}})))
    })
    .await
}

pub(crate) async fn maintenance_mutation(
    state: AppState,
    headers: HeaderMap,
    path: impl Into<String>,
    action: &'static str,
    body: Value,
    work: impl FnOnce(Ctx<'_>) -> Result<(u16, Value), MutationError> + Send + 'static,
) -> Response {
    let path = path.into();
    let error_path = path.clone();
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
            requires_open_gate: false,
            method: "POST",
            path: &path,
            action,
            idempotency_key: &key,
            body: &body,
            absolute_ttl: i64::try_from(state.config.session_absolute_ttl_seconds).unwrap_or(0),
            idle_ttl: i64::try_from(state.config.session_idle_ttl_seconds).unwrap_or(0),
            csrf_ttl: i64::try_from(state.config.csrf_ttl_seconds).unwrap_or(0),
        };
        mutation::execute(ctx, &request, work)
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

//! Browser gate control through the same fenced operator API as the local CLI.
use crate::{
    extract::AuthenticatedSession, mutation::MutationError, read::error, read_jobs::safe_head,
    routes_jobs::maintenance_mutation, state::AppState,
};
use atvprep_manager::{
    deployment_gate::{DeploymentGate, Operator},
    repo::Ctx,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Value, json};

fn unreadable() -> Value {
    json!({"mode":"maintenance","phase":"offline-migration","generation":0,"healthy":false,"reason":"the gate state cannot be read","permits":0,"quiesced":false,"changed_by_kind":null,"changed_at":null,"active_items":0,"active_scans":0,"open_permits":[]})
}

fn view(ctx: Ctx<'_>) -> Value {
    checked_view(ctx).unwrap_or_else(|_| unreadable())
}

fn checked_view(ctx: Ctx<'_>) -> Result<Value, MutationError> {
    let gate = DeploymentGate(ctx);
    let status = gate.status().map_err(|_| MutationError::Database)?;
    if !status.healthy {
        return Ok(unreadable());
    }
    let metadata: Option<(Option<String>, String)> = ctx
        .conn
        .query_row(
            "SELECT changed_by_kind,changed_at FROM deployment_gate WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| MutationError::Database)?;
    let (changed_by_kind, changed_at) =
        metadata.map_or((None, None), |(actor, at)| (actor, Some(at)));
    let items:i64=ctx.conn.query_row("SELECT count(*) FROM job_items WHERE status IN ('claimed','running','cancel_requested','cancelling')",[],|r|r.get(0)).map_err(|_|MutationError::Database)?;
    let scans: i64 = ctx
        .conn
        .query_row(
            "SELECT count(*) FROM scan_runs WHERE status IN ('claimed','running')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| MutationError::Database)?;
    let permits=gate.open_permits(20).map_err(|_|MutationError::Database)?.into_iter().map(|permit|json!({"id":permit.id,"kind":permit.kind,"holder":safe_head(&permit.holder, 200),"pid":permit.pid,"issued_at":permit.issued_at})).collect::<Vec<_>>();
    Ok(
        json!({"mode":status.mode,"phase":status.phase,"generation":status.generation,"healthy":status.healthy,"reason":status.reason.map(|text|safe_head(&text, 800).chars().take(200).collect::<String>()),"permits":status.permits,"quiesced":status.quiesced,"changed_by_kind":changed_by_kind,"changed_at":changed_at,"active_items":items,"active_scans":scans,"open_permits":permits}),
    )
}

fn worker(ctx: Ctx<'_>, table: &str) -> Result<Value, MutationError> {
    let expires: Option<String> = ctx
        .conn
        .query_row(
            &format!("SELECT lease_expires_at FROM {table} WHERE id=1"),
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| MutationError::Database)?
        .flatten();
    let remaining = expires
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(&crate::read::timestamp(&text)).ok())
        .map(|expires| {
            expires
                .timestamp_micros()
                .saturating_sub(ctx.clock.now_micros())
                / 1_000_000
        });
    Ok(
        json!({"alive":remaining.is_some_and(|value|value>0),"expires_in_seconds":remaining.map(|value|value.max(0))}),
    )
}

pub async fn status(State(state): State<AppState>, _session: AuthenticatedSession) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| MutationError::Database)?;
        let ctx = Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        };
        let mut data = view(ctx);
        data["workers"] = match (
            worker(ctx, "scheduler_leases"),
            worker(ctx, "media_executor_leases"),
        ) {
            (Ok(scheduler), Ok(media)) => json!({"scheduler":scheduler,"media":media}),
            _ => Value::Null,
        };
        Ok::<_, MutationError>(json!({"data":data,"meta":{}}))
    })
    .await;
    match result {
        Ok(Ok(data)) => Json(data).into_response(),
        _ => MutationError::Database.response("/api/v1/deployment"),
    }
}

#[derive(Deserialize)]
pub struct Close {
    reason: String,
}
#[derive(Deserialize)]
pub struct Open {
    expected_generation: i64,
}

pub async fn close(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: AuthenticatedSession,
    Json(input): Json<Close>,
) -> Response {
    const PATH: &str = "/api/v1/deployment/close";
    if input.reason.is_empty() || input.reason.chars().count() > 200 {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "reason must contain 1 to 200 printable characters",
            PATH,
        );
    }
    let body = json!({"reason":input.reason});
    maintenance_mutation(state, headers, PATH, "close-gate", body, move |ctx| {
        let operator =
            Operator::from_admin_session(session.id).map_err(|_| MutationError::Unauthorized)?;
        DeploymentGate(ctx)
            .close(&operator, &input.reason)
            .map_err(gate_error)?;
        Ok((200, json!({"data":view(ctx)})))
    })
    .await
}

pub async fn open(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: AuthenticatedSession,
    Json(input): Json<Open>,
) -> Response {
    const PATH: &str = "/api/v1/deployment/open";
    if input.expected_generation < 0 {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "expected generation must be nonnegative",
            PATH,
        );
    }
    let body = json!({"expected_generation":input.expected_generation});
    maintenance_mutation(state, headers, PATH, "open-gate", body, move |ctx| {
        let operator =
            Operator::from_admin_session(session.id).map_err(|_| MutationError::Unauthorized)?;
        DeploymentGate(ctx)
            .open(&operator, input.expected_generation)
            .map_err(gate_error)?;
        Ok((200, json!({"data":view(ctx)})))
    })
    .await
}

// Only fixed, documented gate messages cross the API boundary.
fn gate_error(error: atvprep_manager::Error) -> MutationError {
    if let atvprep_manager::Error::Conflict(message) | atvprep_manager::Error::Invalid(message) =
        error
    {
        for safe in [
            "the reason must be 1 to 200 printable characters",
            "the evidence must be 1 to 200 printable characters",
            "the gate is not healthy; close it to repair it first",
            "the gate changed since you looked; read it again",
            "the gate was closed from the host; open it from the host",
            "the offline migration is not finished",
            "the gate is not in an offline migration",
            "the gate is not closed",
            "work is still running",
            "the gate change was refused",
        ] {
            if message == safe {
                return MutationError::Rejected(safe);
            }
        }
    }
    MutationError::Database
}

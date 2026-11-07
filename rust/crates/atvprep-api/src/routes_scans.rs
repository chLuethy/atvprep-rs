//! Operator-requested library scans.
use crate::{mutation::MutationError, opaque::Opaque, schedule_routes::mutate, state::AppState};
use atvprep_manager::{
    scans::Scans,
    worker::orphans::{self, RemoveOutcome},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct CreateScan {
    root_id: String,
}

#[derive(Deserialize)]
pub struct RemoveOrphans {
    count: usize,
    bytes: u64,
}

/// Delete the leftover partial rewrites of one library after the operator confirmed their count
/// and size. When the files differ from what was confirmed, nothing is deleted and the answer is
/// `409` with the current numbers.
pub async fn remove_orphans(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Json(input): Json<RemoveOrphans>,
) -> Response {
    let body = json!({"root_id":token,"count":input.count,"bytes":input.bytes});
    mutate(
        state,
        headers,
        "POST",
        format!("/api/v1/roots/{token}/orphans/remove"),
        "remove-orphans",
        body,
        move |ctx, state| {
            let root = Opaque::new(state.api_secret.as_bytes())
                .decode(&token, "r")
                .ok()
                .filter(|id| *id > 0)
                .ok_or(MutationError::NotFound)?;
            let exists: bool = ctx
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM library_roots WHERE id=?1)",
                    [root],
                    |row| row.get(0),
                )
                .map_err(|_| MutationError::Database)?;
            if !exists {
                return Err(MutationError::NotFound);
            }
            match orphans::remove(ctx, root, input.count, input.bytes)
                .map_err(|_| MutationError::Database)?
            {
                RemoveOutcome::Removed { count, bytes } => {
                    Ok((200, json!({"data":{"removed":count,"bytes":bytes}})))
                }
                RemoveOutcome::Changed { count, bytes } => {
                    Ok((409, json!({"data":{"count":count,"bytes":bytes}})))
                }
            }
        },
    )
    .await
}

/// Enqueue a manual scan, deduplicating pending requests within a root.
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateScan>,
) -> Response {
    let body = json!({"root_id":input.root_id});
    mutate(
        state,
        headers,
        "POST",
        "/api/v1/scans".to_owned(),
        "scan",
        body,
        move |ctx, state| {
            let opaque = Opaque::new(state.api_secret.as_bytes());
            let root = opaque
                .decode(&input.root_id, "r")
                .ok()
                .filter(|id| *id > 0)
                .ok_or(MutationError::NotFound)?;
            let exists: bool = ctx
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM library_roots WHERE id=?1)",
                    [root],
                    |row| row.get(0),
                )
                .map_err(|_| MutationError::Database)?;
            if !exists {
                return Err(MutationError::NotFound);
            }
            let scan = Scans(ctx)
                .request_manual(root)
                .map_err(|_| MutationError::Database)?;
            Ok((
                201,
                json!({"data":{"id":opaque.encode("s",scan.id),"status":scan.status}}),
            ))
        },
    )
    .await
}

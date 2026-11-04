//! Liveness, readiness and service version routes.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::state::AppState;

pub const SERVICE_NAME: &str = "atvprep-manager";

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}
#[derive(Serialize)]
struct VersionResponse {
    service: &'static str,
    version: &'static str,
}

pub async fn live() -> impl IntoResponse {
    Json(HealthResponse { status: "alive" })
}
pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let spa_ready = if let Some(root) = state.web_dist.clone() {
        tokio::task::spawn_blocking(move || crate::spa::index_available(&root))
            .await
            .unwrap_or(false)
    } else {
        state.embedded_web && crate::spa::embedded_available()
    };
    let probes = state.readiness_probes.clone();
    let workers_ready = tokio::task::spawn_blocking(move || {
        !probes.is_empty() && probes.iter().all(|probe| probe())
    })
    .await
    .unwrap_or(false);
    if !spa_ready || !workers_ready {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "not ready",
            }),
        );
    }
    (StatusCode::OK, Json(HealthResponse { status: "ready" }))
}
pub async fn version() -> impl IntoResponse {
    Json(VersionResponse {
        service: SERVICE_NAME,
        version: env!("CARGO_PKG_VERSION"),
    })
}

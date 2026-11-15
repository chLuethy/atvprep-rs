//! HTTP API for the atvprep manager.

pub mod auth;
pub mod bounds;
pub mod casefold;
pub mod config;
pub mod errors;
pub mod extract;
mod facets;
mod filter_params;
pub mod health;
mod job_filter;
mod job_trigger;
pub mod mutation;
pub mod opaque;
pub mod random;
mod read;
mod read_events;
mod read_files;
mod read_jobs;
pub mod redact;
mod routes_auth;
mod routes_deployment;
mod routes_jobs;
mod routes_scans;
mod schedule_routes;
mod settings_routes;
pub mod spa;
pub mod state;
mod ws;

use axum::Router;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::errors::ProblemDetail;
use crate::state::AppState;

#[allow(clippy::too_many_lines)] // one flat table of routes
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/ws", get(ws::upgrade))
        .route("/api/health/live", get(health::live))
        .route("/api/health/ready", get(health::ready))
        .route("/api/v1/version", get(health::version))
        .route(
            "/api/v1/auth/session",
            get(routes_auth::current)
                .post(routes_auth::bootstrap)
                .delete(routes_auth::revoke),
        )
        .route("/api/v1/auth/csrf", get(routes_auth::csrf))
        .route("/api/v1/deployment", get(routes_deployment::status))
        .route(
            "/api/v1/deployment/close",
            axum::routing::post(routes_deployment::close),
        )
        .route(
            "/api/v1/deployment/open",
            axum::routing::post(routes_deployment::open),
        )
        .route("/api/v1/roots", get(read::roots))
        .route(
            "/api/v1/roots/{root_id}/orphans/remove",
            axum::routing::post(routes_scans::remove_orphans),
        )
        .route("/api/v1/dashboard", get(read::dashboard))
        .route("/api/v1/dashboard/summary", get(read::dashboard))
        .route("/api/v1/dashboard/activity", get(read::activity))
        .route("/api/v1/dashboard/queue", get(read_jobs::queue))
        .route("/api/v1/dashboard/queue-preview", get(read_jobs::queue))
        .route("/api/v1/scans", get(read::scans).post(routes_scans::create))
        .route("/api/v1/scans/{scan_id}", get(read::scan))
        .route(
            "/api/v1/jobs",
            get(read_jobs::jobs).post(routes_jobs::create),
        )
        .route("/api/v1/jobs/counts", get(read_jobs::job_counts))
        .route(
            "/api/v1/jobs/clear",
            axum::routing::post(routes_jobs::clear),
        )
        .route("/api/v1/jobs/{job_id}", get(read_jobs::job))
        .route(
            "/api/v1/jobs/{job_id}/retry",
            axum::routing::post(routes_jobs::retry),
        )
        .route(
            "/api/v1/jobs/{job_id}/cancel",
            axum::routing::post(routes_jobs::cancel),
        )
        .route("/api/v1/jobs/{job_id}/items", get(read_jobs::items))
        .route("/api/v1/jobs/{job_id}/events", get(read_events::job_events))
        .route("/api/v1/events", get(read_events::events))
        .route("/api/v1/settings/audit", get(settings_routes::audit))
        .route(
            "/api/v1/settings",
            get(settings_routes::list).patch(settings_routes::patch),
        )
        .route(
            "/api/v1/settings/{key}",
            axum::routing::put(settings_routes::put),
        )
        .route(
            "/api/v1/schedules",
            get(schedule_routes::list).post(schedule_routes::create),
        )
        .route(
            "/api/v1/schedules/cron-preview",
            get(schedule_routes::cron_preview),
        )
        .route(
            "/api/v1/schedules/{schedule_id}",
            axum::routing::delete(schedule_routes::delete).patch(schedule_routes::update),
        )
        .route(
            "/api/v1/schedules/{schedule_id}/run",
            axum::routing::post(schedule_routes::run),
        )
        .route("/api/v1/job-items/{item_id}/log", get(read_jobs::item_log))
        .route(
            "/api/v1/job-items/{item_id}/raise-encode",
            axum::routing::post(routes_jobs::raise_encode),
        )
        .route("/api/v1/files/counts", get(read_files::facet_counts))
        .route("/api/v1/files/{media_id}/jobs", get(read_jobs::file_jobs))
        .route("/api/v1/files", get(read_files::list))
        .route(
            "/api/v1/files/dismiss-missing",
            axum::routing::post(routes_jobs::dismiss_missing),
        )
        .route("/api/v1/browse", get(read_files::browse))
        .route("/api/v1/browse/below", get(read_files::browse_below))
        .route("/api/v1/files/{media_id}", get(read_files::detail))
        .route(
            "/api/v1/files/{media_id}/inspect",
            axum::routing::post(routes_jobs::inspect_file),
        )
        .route(
            "/api/v1/files/{media_id}/inspections",
            get(read_files::inspections),
        )
        .route(
            "/api/v1/files/{media_id}/sidecars",
            get(read_files::sidecars),
        )
        .route("/docs", get(disabled_docs))
        .route("/redoc", get(disabled_docs))
        .route("/openapi.json", get(disabled_docs))
        .fallback(fallback)
        .layer(middleware::from_fn(api_policy))
        .with_state(state)
}

async fn disabled_docs() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({"detail": "not found"})),
    )
}

async fn fallback(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
) -> Response {
    let path = request.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        return ProblemDetail::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found",
            path,
        )
        .into_response();
    }
    if let Some(root) = state.web_dist.as_ref() {
        return spa::serve(root.clone(), request).await;
    }
    if state.embedded_web && spa::embedded_available() {
        return spa::serve_embedded(request).await;
    }
    ProblemDetail::new(
        StatusCode::NOT_FOUND,
        "not_found",
        &format!("No route registered for {} {path}", request.method()),
        path,
    )
    .into_response()
}

async fn api_policy(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    if path.starts_with("/api/v1")
        && axum::extract::Query::<std::collections::HashMap<String, String>>::try_from_uri(
            request.uri(),
        )
        .is_ok_and(|query| {
            query
                .keys()
                .any(|key| matches!(key.as_str(), "page" | "page_size" | "offset"))
        })
    {
        let mut response = ProblemDetail::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "page and offset pagination are not supported",
            &path,
        )
        .into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        return response;
    }
    let response = next.run(request).await;
    let mut response = if path.starts_with("/api/") {
        errors::normalise_response(response, &path).await
    } else {
        response
    };
    if path.starts_with("/api/v1") || path.starts_with("/api/health/") {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
    }
    response
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::state;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_ready_is_503_without_probes() {
        let response = router(state())
            .oneshot(
                axum::http::Request::get("/api/health/ready")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[tokio::test]
    async fn unknown_api_path_is_a_problem_json_404() {
        let response = router(state())
            .oneshot(
                axum::http::Request::get("/api/nope")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert!(std::str::from_utf8(&body).unwrap().contains("not_found"));
    }

    #[tokio::test]
    async fn docs_are_disabled() {
        let response = router(state())
            .oneshot(
                axum::http::Request::get("/docs")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pagination_params_are_rejected() {
        let response = router(state())
            .oneshot(
                axum::http::Request::get("/api/v1/version?page=1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

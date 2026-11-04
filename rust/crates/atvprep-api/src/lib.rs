//! HTTP API for the atvprep manager.

pub mod auth;
pub mod bounds;
pub mod casefold;
pub mod config;
pub mod errors;
pub mod extract;
pub mod health;
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
mod spa;
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
            "/api/v1/schedules/{schedule_id}",
            axum::routing::delete(schedule_routes::delete).patch(schedule_routes::update),
        )
        .route(
            "/api/v1/schedules/{schedule_id}/run",
            axum::routing::post(schedule_routes::run),
        )
        .route("/api/v1/job-items/{item_id}/log", get(read_jobs::item_log))
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
mod tests {
    use super::*;
    use crate::config::ManagerConfig;
    use crate::state::ReadinessProbe;
    use axum::body::to_bytes;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state() -> AppState {
        let config = ManagerConfig {
            api_secret: Some("secret".to_owned()),
            allowed_origins: vec!["http://localhost".to_owned()],
            ..ManagerConfig::default()
        };
        AppState::new(atvprep_manager::db::Db::memory().unwrap(), config).unwrap()
    }
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
    #[test]
    fn startup_rejects_insecure_cookies_on_a_public_bind() {
        let config = ManagerConfig {
            api_secret: Some("secret".to_owned()),
            allowed_origins: vec!["http://localhost".to_owned()],
            bind_host: "0.0.0.0".to_owned(),
            session_cookie_secure: false,
            ..ManagerConfig::default()
        };
        let Err(error) = AppState::new(atvprep_manager::db::Db::memory().unwrap(), config) else {
            panic!("public insecure cookies must be rejected");
        };
        assert_eq!(
            error,
            crate::state::StartupError::InsecureCookiesOnPublicBind
        );
    }
    #[test]
    fn a_true_probe_makes_ready() {
        let probe: ReadinessProbe = Arc::new(|| true);
        let result = state().with_readiness_probes(vec![probe]);
        assert_eq!(result.readiness_probes.len(), 1);
    }

    #[tokio::test]
    async fn session_bootstrap_and_logout_require_origin_and_single_use_csrf() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let app = router(state);
        let request = || {
            axum::http::Request::post("/api/v1/auth/session")
                .header("authorization", "Bearer secret")
        };
        let denied = app
            .clone()
            .oneshot(request().body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let response = app
            .clone()
            .oneshot(
                request()
                    .header("origin", "http://localhost")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let csrf = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["csrf_token"]
            .as_str()
            .unwrap()
            .to_owned();
        let logout = || {
            axum::http::Request::delete("/api/v1/auth/session")
                .header("origin", "http://localhost")
                .header("cookie", &cookie)
                .header("x-csrf-token", &csrf)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        assert_eq!(
            app.clone().oneshot(logout()).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            app.oneshot(logout()).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn authenticated_extractor_checks_session_and_origin() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let cookie = format!("__Host-atvprep_session={token}");
        let app = Router::new()
            .route(
                "/protected",
                get(|_: crate::extract::AuthenticatedSession| async { StatusCode::OK }),
            )
            .with_state(state);
        let request = |origin: Option<&str>, cookie: Option<&str>| {
            let mut request = axum::http::Request::get("/protected");
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            if let Some(cookie) = cookie {
                request = request.header("cookie", cookie);
            }
            request.body(axum::body::Body::empty()).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request(Some("http://localhost"), None))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request(Some("http://evil"), Some(&cookie)))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(request(None, Some(&cookie)))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.oneshot(request(Some("http://localhost"), Some(&cookie)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn roots_returns_latest_scan_without_the_private_path() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private/library'),(2,'/secret/other','/secret/other'); INSERT INTO scan_runs(id,root_id,generation,status) VALUES(1,1,1,'failed'),(2,1,2,'succeeded');").unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let response = app
            .oneshot(
                axum::http::Request::get("/api/v1/roots")
                    .header("origin", "http://localhost")
                    .header("cookie", format!("__Host-atvprep_session={token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["data"][0]["name"], "Movies");
        assert_eq!(body["data"][1]["name"], "other");
        assert_eq!(body["data"][0]["latest_scan"]["generation"], 2);
        assert_eq!(
            body["data"][0]["latest_scan"]["root_id"],
            body["data"][0]["id"]
        );
        assert!(!body.to_string().contains("/private/library"));
        assert!(!body.to_string().contains("/secret"));
    }

    #[tokio::test]
    async fn dashboard_counts_converted_missing_and_pending_files() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private');
        INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action,compliance_reasons,dismissed_at) VALUES
        (1,1,'film.mkv','video','.mkv',1,1,'a','present','warning','idle','none','[\"[video] video-reencode: required\"]',NULL),
        (2,1,'film.mp4','video','.mp4',1,1,'b','present','clean','idle','none','[]',NULL),
        (3,1,'other.mkv','video','.mkv',1,1,'c','present','unknown','idle','none','[]',NULL),
        (4,1,'gone.mkv','video','.mkv',1,1,'d','missing','unknown','idle','none','[]',NULL),
        (5,1,'dismissed.mkv','video','.mkv',1,1,'e','missing','unknown','idle','none','[]','2026-01-01');
        INSERT INTO jobs(action,status) VALUES('verify','queued'); INSERT INTO scan_runs(root_id,generation,status) VALUES(1,1,'succeeded');
        INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original','true','bool',1);").unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let response = router(state)
            .oneshot(
                axum::http::Request::get("/api/v1/dashboard/summary")
                    .header("origin", "http://localhost")
                    .header("cookie", format!("__Host-atvprep_session={token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"]["missing"], 1);
        assert_eq!(body["data"]["convert"], 1);
        assert_eq!(body["data"]["converted"], 1);
        assert_eq!(body["data"]["reencode"], 1);
        assert_eq!(body["data"]["files"]["unknown"], 2);
        assert_eq!(body["data"]["delete_original"], true);
        assert_eq!(body["data"]["jobs"]["queued"], 1);
        assert_eq!(body["data"]["scans"]["succeeded"], 1);
    }

    #[tokio::test]
    async fn schedules_count_only_eligible_unblocked_files() {
        let mut state = state();
        state.clock = Arc::new(atvprep_manager::clock::FixedClock(1_700_000_000_000_000));
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private'); INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'a.mp4','video','.mp4',1,1,'a','present','clean','idle','none'),(2,1,'b.mp4','video','.mp4',1,1,'b','present','clean','idle','none'),(3,1,'c.mkv','video','.mkv',1,1,'c','present','clean','idle','none'); INSERT INTO jobs(id,action,status) VALUES(1,'fix','running'); INSERT INTO job_items(job_id,media_file_id,action,position,status) VALUES(1,2,'fix',0,'running'); INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled) VALUES(1,'Weekly','verify',1,127,'23:59',25,1);").unwrap();
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
            serde_json::json!({"name":"Unsafe","action":"fix","days":[0],"time":"12:30"});
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
    async fn schedule_run_and_delete_are_idempotent_and_non_destructive() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private'); INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mp4','video','.mp4',1,1,'a','present','clean','idle','none'); INSERT INTO schedules(id,name,action,days_mask,time_of_day,enabled) VALUES(1,'Verify','verify',1,'12:30',1),(2,'Legacy','fix',1,'12:30',0);").unwrap();
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
            StatusCode::CONFLICT
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
    async fn schedule_updates_rearm_and_reject_legacy_destructive_actions() {
        let mut state = state();
        state.clock = Arc::new(atvprep_manager::clock::FixedClock(1_700_000_000_000_000));
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private'); INSERT INTO schedules(id,name,action,root_id,compliance_state,days_mask,time_of_day,enabled,armed_at) VALUES(1,'Old','verify',1,'clean',1,'12:30',0,'2020-01-01 00:00:00'),(2,'Legacy','fix',NULL,NULL,1,'12:30',0,'2020-01-01 00:00:00');").unwrap();
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
    async fn settings_mutations_are_revisioned_idempotent_and_audited() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let auth = routes_auth::service(&state, &conn).unwrap();
        let session = auth.create().unwrap();
        let patch_csrf = auth
            .issue_csrf(&session.token, "PATCH", "/api/v1/settings", "settings")
            .unwrap();
        let put_csrf = auth
            .issue_csrf(
                &session.token,
                "PUT",
                "/api/v1/settings/max_probe_workers",
                "settings",
            )
            .unwrap();
        let app = router(state.clone());
        let request = |method: axum::http::Method,
                       path: &str,
                       csrf: &str,
                       key: &str,
                       body: serde_json::Value| {
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
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        };
        let patch_body = serde_json::json!({"settings":{"scan_interval_seconds":600,"auto_transcode":true},"expected_revision":0});
        let response = app
            .clone()
            .oneshot(request(
                axum::http::Method::PATCH,
                "/api/v1/settings",
                &patch_csrf,
                "patch",
                patch_body.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = to_bytes(response.into_body(), 65536).await.unwrap();
        let replay = app
            .clone()
            .oneshot(request(
                axum::http::Method::PATCH,
                "/api/v1/settings",
                &patch_csrf,
                "patch",
                patch_body,
            ))
            .await
            .unwrap();
        assert_eq!(first, to_bytes(replay.into_body(), 65536).await.unwrap());
        let body = serde_json::json!({"value":4});
        let mut put = request(
            axum::http::Method::PUT,
            "/api/v1/settings/max_probe_workers",
            &put_csrf,
            "put",
            body,
        );
        put.headers_mut()
            .insert(header::IF_MATCH, "\"1\"".parse().unwrap());
        let response = app.clone().oneshot(put).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let counts = || {
            conn.raw().query_row("SELECT (SELECT revision FROM settings_revision WHERE id=1),(SELECT count(*) FROM settings_audit)", [], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?))).unwrap()
        };
        assert_eq!(counts(), (2, 3));
        let csrf = auth
            .issue_csrf(
                &session.token,
                "PUT",
                "/api/v1/settings/max_probe_workers",
                "settings",
            )
            .unwrap();
        let conflict = app
            .oneshot(request(
                axum::http::Method::PUT,
                "/api/v1/settings/max_probe_workers",
                &csrf,
                "stale",
                serde_json::json!({"value":5,"expected_revision":0}),
            ))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(counts(), (2, 3));
    }

    #[tokio::test]
    async fn dashboard_activity_paginates_events_with_equal_timestamps() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO jobs(id,action,status) VALUES(1,'verify','queued'); INSERT INTO events(id,job_id,topic,entity_type,entity_id,payload_json,created_at) VALUES(1,1,'first','job',1,'{}','2026-11-01 12:00:00'),(2,1,'second','job',1,'{}','2026-11-01 12:00:00'),(3,NULL,'excluded','job',1,'{}','2026-11-01 12:00:00');").unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get("/api/v1/dashboard/activity?limit=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["topic"], "second");
        let cursor = body["meta"]["next_cursor"].as_str().unwrap();
        let response = app
            .oneshot(get(&format!(
                "/api/v1/dashboard/activity?limit=1&cursor={cursor}"
            )))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["topic"], "first");
        assert!(body["meta"]["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn global_events_coalesce_logs_without_skipping_next_page() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private'); INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mp4','video','.mp4',1,1,'a','present','clean','idle','none'); INSERT INTO jobs(id,action,status) VALUES(1,'verify','queued'); INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'verify',0,'queued'); INSERT INTO events(id,job_id,item_id,topic,entity_type,entity_id,payload_json,created_at) VALUES(1,1,1,'job.log','job_item',1,'{}','2026-01-01 00:00:00.000000'),(2,1,1,'job.log','job_item',1,'{}','2026-01-01 00:00:00.100000'),(3,1,1,'job.progress','job_item',1,'{}','2026-01-01 00:00:01.000000');").unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get("/api/v1/events?limit=1"))
            .await
            .unwrap();
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(first["data"][0]["sequence"], 2);
        assert_eq!(first["meta"]["current_sequence"], 3);
        let cursor = first["meta"]["next_cursor"].as_str().unwrap();
        let response = app
            .oneshot(get(&format!("/api/v1/events?limit=1&after={cursor}")))
            .await
            .unwrap();
        let second: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(second["data"][0]["sequence"], 3);
        assert!(second["meta"]["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn roots_truncates_an_oversized_entry() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw()
            .execute(
                "INSERT INTO library_roots(name,canonical_path) VALUES(?1,'/library')",
                ["x".repeat(crate::bounds::MAX_ELEMENT_BYTES)],
            )
            .unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let response = router(state)
            .oneshot(
                axum::http::Request::get("/api/v1/roots")
                    .header("origin", "http://localhost")
                    .header("cookie", format!("__Host-atvprep_session={token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), crate::bounds::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["data"], serde_json::json!([]));
        assert_eq!(body["meta"]["truncated"], true);
    }

    #[tokio::test]
    async fn scans_page_and_detail_use_authenticated_endpoint_bound_tokens() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library'); INSERT INTO scan_runs(id,root_id,generation,status) VALUES(1,1,1,'failed'),(2,1,2,'succeeded');").unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let first = app
            .clone()
            .oneshot(get("/api/v1/scans?limit=1"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(first.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(first["data"][0]["generation"], 1);
        assert_eq!(
            first["meta"].get("current_cursor"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            first["meta"].get("current_sequence"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(first["meta"]["facets"], serde_json::json!({}));
        let cursor = first["meta"]["next_cursor"].as_str().unwrap();
        let second = app
            .clone()
            .oneshot(get(&format!("/api/v1/scans?limit=1&cursor={cursor}")))
            .await
            .unwrap();
        let second: serde_json::Value =
            serde_json::from_slice(&to_bytes(second.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(second["data"][0]["generation"], 2);
        assert!(second["meta"]["next_cursor"].is_null());
        let id = second["data"][0]["id"].as_str().unwrap();
        let detail = app
            .clone()
            .oneshot(get(&format!("/api/v1/scans/{id}")))
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);
        assert_eq!(
            app.clone()
                .oneshot(get(&format!("/api/v1/scans?cursor={id}")))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        conn.raw()
            .execute(
                "UPDATE scan_runs SET finished_at=?1 WHERE id=2",
                ["x".repeat(crate::bounds::MAX_ELEMENT_BYTES)],
            )
            .unwrap();
        let bounded = app.oneshot(get("/api/v1/scans?limit=2")).await.unwrap();
        let bounded: serde_json::Value = serde_json::from_slice(
            &to_bytes(bounded.into_body(), crate::bounds::MAX_RESPONSE_BYTES)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(bounded["data"].as_array().unwrap().len(), 2);
        assert_eq!(bounded["data"][1]["finished_at"], "[invalid timestamp]");
        assert!(bounded["meta"]["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn jobs_filter_page_and_show_prepare_snapshot() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO jobs(id,action,status,settings_snapshot_json) VALUES(1,'fix','queued','{\"pipeline\":\"prepare\",\"delete_original_confirmed\":true}'),(2,'verify','succeeded','{}'),(3,'convert','queued','{}'); UPDATE jobs SET cleared_at='2026-01-01' WHERE id=3;").unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get("/api/v1/jobs?limit=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["action"], "prepare");
        assert_eq!(body["data"][0]["delete_confirmed"], true);
        let id = body["data"][0]["id"].as_str().unwrap();
        let detail = app
            .clone()
            .oneshot(get(&format!("/api/v1/jobs/{id}")))
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);
        let cursor = body["meta"]["next_cursor"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(get(&format!("/api/v1/jobs?limit=1&cursor={cursor}")))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["action"], "verify");
        assert!(body["meta"]["next_cursor"].is_null());
        assert_eq!(
            app.oneshot(get(&format!("/api/v1/jobs?cursor={id}")))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn dashboard_queue_reports_total_and_paginates_active_jobs() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO jobs(id,action,status) VALUES(1,'verify','queued'),(2,'verify','running'),(3,'verify','succeeded');").unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get("/api/v1/dashboard/queue?limit=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"]["total"], 2);
        assert_eq!(body["data"]["jobs"][0]["status"], "queued");
        let cursor = body["meta"]["next_cursor"].as_str().unwrap();
        let response = app
            .oneshot(get(&format!(
                "/api/v1/dashboard/queue-preview?limit=1&cursor={cursor}"
            )))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"]["jobs"][0]["status"], "running");
        assert!(body["meta"]["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn job_items_page_across_equal_positions_without_exposing_paths() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private'); INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'/private/one.mkv','video','.mkv',1,1,'a','present','unknown','idle','none'),(2,1,'two.mkv','video','.mkv',1,1,'b','present','unknown','idle','none'); INSERT INTO jobs(id,action,status) VALUES(1,'verify','queued'); INSERT INTO job_items(id,job_id,media_file_id,action,position,error_detail) VALUES(1,1,1,'verify',0,'failed /private/one.mkv'),(2,1,2,'verify',0,NULL);").unwrap();
        let job = crate::opaque::Opaque::new(state.api_secret.as_bytes()).encode("j", 1);
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let first = app
            .clone()
            .oneshot(get(&format!("/api/v1/jobs/{job}/items?limit=1")))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(first.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(first["data"][0]["display_path"], "one.mkv");
        assert!(!first.to_string().contains("/private"));
        let cursor = first["meta"]["next_cursor"].as_str().unwrap();
        let second = app
            .oneshot(get(&format!(
                "/api/v1/jobs/{job}/items?limit=1&cursor={cursor}"
            )))
            .await
            .unwrap();
        let second: serde_json::Value =
            serde_json::from_slice(&to_bytes(second.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(second["data"][0]["display_path"], "two.mkv");
        assert!(second["meta"]["next_cursor"].is_null());
    }

    #[tokio::test]
    async fn job_item_logs_are_bounded_and_redact_credentials() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private'); INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mkv','video','.mkv',1,1,'a','present','unknown','idle','none'); INSERT INTO jobs(id,action,status) VALUES(1,'verify','queued'); INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'verify',0,'queued');").unwrap();
        conn.raw()
            .execute(
                "UPDATE job_items SET log_tail=?1 WHERE id=1",
                ["é".repeat(5000)],
            )
            .unwrap();
        let id = crate::opaque::Opaque::new(state.api_secret.as_bytes()).encode("i", 1);
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = || {
            axum::http::Request::get(format!("/api/v1/job-items/{id}/log"))
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app.clone().oneshot(get()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["meta"]["truncated"], true);
        assert!(body["data"]["log_tail"].as_str().unwrap().len() <= 8192);
        conn.raw()
            .execute(
                "UPDATE job_items SET log_tail='token=private-value' WHERE id=1",
                [],
            )
            .unwrap();
        let response = app.oneshot(get()).await.unwrap();
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        assert!(
            !std::str::from_utf8(&body)
                .unwrap()
                .contains("private-value")
        );
    }

    #[tokio::test]
    async fn sidecars_classify_current_probes_and_page_backwards() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private'); INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'film.mp4','video','.mp4',1,1,'current','present','unknown','idle','none'); INSERT INTO sidecars(id,media_file_id,relative_path,fingerprint) VALUES(1,1,'/secret/film.en.srt','a'),(2,1,'film.de.srt','b'); INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result) VALUES(1,'source','current','ffprobe','1','{\"streams\":[]}');").unwrap();
        let media = crate::opaque::Opaque::new(state.api_secret.as_bytes()).encode("m", 1);
        let session = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let app = router(state);
        let get = |path: &str| {
            axum::http::Request::get(path)
                .header("origin", "http://localhost")
                .header("cookie", format!("__Host-atvprep_session={session}"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(get(&format!("/api/v1/files/{media}/sidecars?limit=1")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["name"], "film.de.srt");
        assert_eq!(body["data"][0]["state"], "pending");
        let cursor = body["meta"]["next_cursor"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(get(&format!(
                "/api/v1/files/{media}/sidecars?limit=1&cursor={cursor}"
            )))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"][0]["name"], "film.en.srt");
        assert!(!body.to_string().contains("/secret"));
        conn.raw().execute_batch("INSERT INTO jobs(id,action,status) VALUES(1,'fix','running'); INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'fix',0,'running');").unwrap();
        let response = app
            .oneshot(get(&format!("/api/v1/files/{media}")))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["data"]["fix_status"], "running");
        assert_eq!(body["data"]["pending_sidecars"], 2);
        assert_eq!(body["data"]["sidecar_count"], 2);
        assert_eq!(body["data"]["inspection_available"], true);
        assert!(!body.to_string().contains("/secret"));
    }

    #[tokio::test]
    async fn spa_serving_keeps_api_separate_and_rejects_symlink_assets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("index.html"), "index").unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "app").unwrap();
        std::fs::write(dir.path().join("favicon.svg"), "<svg/>").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            dir.path().join("index.html"),
            dir.path().join("assets/link.js"),
        )
        .unwrap();
        let app = router(state().with_web_dist(dir.path().to_path_buf()));
        let get = |path| {
            axum::http::Request::get(path)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let index = app.clone().oneshot(get("/files/123")).await.unwrap();
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(index.headers()[header::CACHE_CONTROL], "no-cache");
        let asset = app.clone().oneshot(get("/assets/app.js")).await.unwrap();
        assert_eq!(
            asset.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let favicon = app.clone().oneshot(get("/favicon.svg")).await.unwrap();
        assert_eq!(favicon.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(favicon.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(
            app.clone()
                .oneshot(get("/missing.svg"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.clone().oneshot(get("/api")).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.clone()
                .oneshot(get("/api/unknown"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.clone()
                .oneshot(get("/assets/%2e%2e/index.html"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        #[cfg(unix)]
        assert_eq!(
            app.oneshot(get("/assets/link.js")).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn readiness_requires_a_safe_readable_spa_index() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(
            state()
                .with_readiness_probes(vec![Arc::new(|| true)])
                .with_web_dist(dir.path().to_path_buf()),
        );
        let ready = || {
            axum::http::Request::get("/api/health/ready")
                .body(axum::body::Body::empty())
                .unwrap()
        };
        assert_eq!(
            app.clone().oneshot(ready()).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        std::fs::write(dir.path().join("index.html"), "index").unwrap();
        assert_eq!(
            app.clone().oneshot(ready()).await.unwrap().status(),
            StatusCode::OK
        );
        std::fs::remove_file(dir.path().join("index.html")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("missing", dir.path().join("index.html")).unwrap();
        assert_eq!(
            app.oneshot(ready()).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

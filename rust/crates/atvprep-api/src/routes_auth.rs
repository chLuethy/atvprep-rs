//! Browser authentication endpoints.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    auth::{AuthService, SessionCookiePolicy, verify_master_token},
    errors::ProblemDetail,
    state::AppState,
};
use atvprep_manager::repo::Ctx;

const SESSION_PATH: &str = "/api/v1/auth/session";

pub(crate) fn allowed(headers: &HeaderMap, state: &AppState) -> bool {
    match headers.get(header::ORIGIN) {
        Some(origin) => origin.to_str().is_ok_and(|v| {
            state
                .config
                .allowed_origins
                .iter()
                .any(|allowed| allowed == v)
        }),
        None => {
            !state.config.allowed_origins.is_empty()
                && headers
                    .get("sec-fetch-site")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| matches!(v, "same-origin" | "none"))
        }
    }
}

pub(crate) fn failure(status: StatusCode, path: &str) -> Response {
    let (code, message) = if status == StatusCode::FORBIDDEN {
        ("forbidden", "origin not allowed")
    } else {
        ("unauthorized", "authentication required")
    };
    ProblemDetail::new(status, code, message, path).into_response()
}

pub(crate) fn cookie(headers: &HeaderMap, secure: bool) -> Option<String> {
    let name = SessionCookiePolicy::new(secure).name;
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|entry| entry.trim().split_once('='))
        .find_map(|(key, value)| (key == name && !value.is_empty()).then(|| value.to_owned()))
}

pub(crate) fn service<'a>(
    state: &'a AppState,
    conn: &'a atvprep_manager::db::Conn,
) -> Result<AuthService<'a>, ()> {
    let config = &state.config;
    AuthService::with_csrf_ttl(
        Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        },
        i64::try_from(config.session_absolute_ttl_seconds).map_err(|_| ())?,
        i64::try_from(config.session_idle_ttl_seconds).map_err(|_| ())?,
        i64::try_from(config.csrf_ttl_seconds).map_err(|_| ())?,
    )
    .map_err(|_| ())
}

pub async fn bootstrap(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !allowed(&headers, &state) {
        return failure(StatusCode::FORBIDDEN, SESSION_PATH);
    }
    let candidate = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && !token.is_empty())
        .map(|(_, token)| token);
    if !candidate.is_some_and(|candidate| verify_master_token(candidate, &state.api_secret)) {
        return failure(StatusCode::UNAUTHORIZED, SESSION_PATH);
    }
    let secure = state.config.session_cookie_secure;
    let issued = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        service(&state, &conn)?.create().map_err(|_| ())
    })
    .await;
    let Ok(Ok(issued)) = issued else {
        return failure(StatusCode::UNAUTHORIZED, SESSION_PATH);
    };
    let policy = SessionCookiePolicy::new(secure);
    let value = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict{}",
        policy.name,
        issued.token,
        if secure { "; Secure" } else { "" }
    );
    let mut response = Json(json!({"csrf_token": issued.csrf_token})).into_response();
    if let Ok(value) = value.parse() {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

pub async fn current(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !allowed(&headers, &state) {
        return failure(StatusCode::FORBIDDEN, SESSION_PATH);
    }
    let Some(token) = cookie(&headers, state.config.session_cookie_secure) else {
        return failure(StatusCode::UNAUTHORIZED, SESSION_PATH);
    };
    let csrf = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        service(&state, &conn)?
            .issue_csrf(&token, "DELETE", SESSION_PATH, "auth-session")
            .map_err(|_| ())
    })
    .await;
    match csrf {
        Ok(Ok(csrf)) => Json(json!({"csrf_token": csrf})).into_response(),
        _ => failure(StatusCode::UNAUTHORIZED, SESSION_PATH),
    }
}

#[derive(Deserialize)]
pub struct CsrfRequest {
    method: String,
    path: String,
    action: String,
}

pub async fn csrf(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CsrfRequest>,
) -> Response {
    const PATH: &str = "/api/v1/auth/csrf";
    if !allowed(&headers, &state) {
        return failure(StatusCode::FORBIDDEN, PATH);
    }
    let Some(token) = cookie(&headers, state.config.session_cookie_secure) else {
        return failure(StatusCode::UNAUTHORIZED, PATH);
    };
    if matches!(
        query.method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    ) || !query.path.starts_with("/api/v1/")
    {
        return failure(StatusCode::UNAUTHORIZED, PATH);
    }
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.conn().map_err(|_| ())?;
        service(&state, &conn)?
            .issue_csrf(&token, &query.method, &query.path, &query.action)
            .map_err(|_| ())
    })
    .await;
    match result {
        Ok(Ok(csrf)) => Json(json!({"csrf_token": csrf})).into_response(),
        _ => failure(StatusCode::UNAUTHORIZED, PATH),
    }
}

pub async fn revoke(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !allowed(&headers, &state) {
        return failure(StatusCode::FORBIDDEN, SESSION_PATH);
    }
    let Some(token) = cookie(&headers, state.config.session_cookie_secure) else {
        return failure(StatusCode::UNAUTHORIZED, SESSION_PATH);
    };
    let Some(csrf) = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return failure(StatusCode::UNAUTHORIZED, SESSION_PATH);
    };
    let secure = state.config.session_cookie_secure;
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = state.db.conn().map_err(|_| ())?;
        conn.transaction(|tx| {
            let svc = AuthService::with_csrf_ttl(
                Ctx {
                    conn: tx,
                    clock: state.clock.as_ref(),
                },
                i64::try_from(state.config.session_absolute_ttl_seconds).unwrap_or(0),
                i64::try_from(state.config.session_idle_ttl_seconds).unwrap_or(0),
                i64::try_from(state.config.csrf_ttl_seconds).unwrap_or(0),
            )
            .map_err(|_| atvprep_manager::Error::Invalid("authentication failed".into()))?;
            svc.validate_csrf(&token, &csrf, "DELETE", SESSION_PATH, "auth-session", false)
                .map_err(|_| atvprep_manager::Error::Invalid("authentication failed".into()))?;
            svc.revoke(&token)
                .map_err(|_| atvprep_manager::Error::Invalid("authentication failed".into()))
        })
        .map_err(|_| ())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        return failure(StatusCode::UNAUTHORIZED, SESSION_PATH);
    }
    let mut response = Json(json!({"data": {"revoked": true}})).into_response();
    let value = format!(
        "{}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}",
        SessionCookiePolicy::new(secure).name,
        if secure { "; Secure" } else { "" }
    );
    if let Ok(value) = value.parse() {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

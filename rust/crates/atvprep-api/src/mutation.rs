//! Persistent idempotency reservations for authenticated mutations.

use atvprep_manager::{clock::format_micros, repo::Ctx};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq, Eq)]
pub enum Reservation {
    New,
    Replay { status: u16, body: Value },
}

#[derive(Debug, PartialEq, Eq)]
pub enum MutationError {
    InvalidKey,
    Conflict,
    InProgress,
    Unauthorized,
    Forbidden,
    Csrf,
    GateClosed,
    InvalidInput(&'static str),
    Validation(String),
    NotFound,
    Rejected(&'static str),
    Database,
}

impl MutationError {
    /// Convert a mutation failure into the public API error format.
    #[must_use]
    pub fn response(&self, path: &str) -> axum::response::Response {
        use axum::{
            http::{HeaderValue, StatusCode, header},
            response::IntoResponse,
        };
        let (status, code, message) = match self {
            Self::InvalidKey => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Idempotency-Key is required",
            ),
            Self::Conflict => (
                StatusCode::CONFLICT,
                "conflict",
                "idempotency key conflicts with request body",
            ),
            Self::InProgress => (
                StatusCode::CONFLICT,
                "conflict",
                "request is already in progress",
            ),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication required",
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "origin not allowed"),
            Self::Csrf => (StatusCode::FORBIDDEN, "forbidden", "CSRF validation failed"),
            Self::GateClosed => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "maintenance: new work is not accepted right now",
            ),
            Self::InvalidInput(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                *message,
            ),
            Self::Validation(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                message.as_str(),
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "resource not found"),
            Self::Rejected(message) => (StatusCode::CONFLICT, "conflict", *message),
            Self::Database => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "internal server error",
            ),
        };
        let mut response =
            crate::errors::ProblemDetail::new(status, code, message, path).into_response();
        if *self == Self::GateClosed {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        }
        response
    }
}

/// Credentials and request metadata required for one mutation.
pub struct MutationRequest<'a> {
    pub token: &'a str,
    pub csrf: &'a str,
    pub origin_allowed: bool,
    /// Set for new work; cancellation must remain available during maintenance.
    pub requires_open_gate: bool,
    pub method: &'a str,
    pub path: &'a str,
    pub action: &'a str,
    pub idempotency_key: &'a str,
    pub body: &'a Value,
    pub absolute_ttl: i64,
    pub idle_ttl: i64,
    pub csrf_ttl: i64,
}

/// Authenticate, validate CSRF, reserve the request and execute it atomically.
/// Completed replays bypass the gate and never repeat the supplied operation.
///
/// # Errors
/// Returns an authentication, CSRF, gate, idempotency or database error.
pub fn execute(
    ctx: Ctx<'_>,
    request: &MutationRequest<'_>,
    work: impl FnOnce(Ctx<'_>) -> Result<(u16, Value), MutationError>,
) -> Result<(u16, Value), MutationError> {
    let tx = atvprep_manager::db::begin_immediate(ctx.conn).map_err(|_| MutationError::Database)?;
    let tx_ctx = Ctx {
        conn: &tx,
        clock: ctx.clock,
    };
    let auth = crate::auth::AuthService::with_csrf_ttl(
        tx_ctx,
        request.absolute_ttl,
        request.idle_ttl,
        request.csrf_ttl,
    )
    .map_err(|_| MutationError::Database)?;
    let session_id = auth
        .authenticate(request.token)
        .map_err(|_| MutationError::Unauthorized)?;
    if !request.origin_allowed {
        return Err(MutationError::Forbidden);
    }
    let key = MutationKey {
        session_id,
        method: request.method,
        path: request.path,
        key: request.idempotency_key,
    };
    let completed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM idempotency_records WHERE auth_session_id=?1 AND method=?2 AND normalized_route=?3 AND key=?4 AND response_status != 0)",
        params![session_id, request.method, request.path, request.idempotency_key], |row| row.get(0),
    ).map_err(|_| MutationError::Database)?;
    auth.validate_csrf(
        request.token,
        request.csrf,
        request.method,
        request.path,
        request.action,
        completed,
    )
    .map_err(|_| MutationError::Csrf)?;
    let result = match reserve(tx_ctx, &key, request.body)? {
        Reservation::Replay { status, body } => (status, body),
        Reservation::New => {
            if request.requires_open_gate {
                let gate = atvprep_manager::deployment_gate::DeploymentGate(tx_ctx)
                    .status()
                    .map_err(|_| MutationError::GateClosed)?;
                if !gate.healthy || gate.mode != "open" || gate.phase != "steady" {
                    return Err(MutationError::GateClosed);
                }
            }
            let (status, body) = work(tx_ctx)?;
            let body = bounded_response(&body);
            store(tx_ctx, &key, status, &body)?;
            (status, body)
        }
    };
    tx.commit().map_err(|_| MutationError::Database)?;
    Ok(result)
}

fn bounded_response(body: &Value) -> Value {
    if serde_json::to_vec(body).map_or(true, |encoded| {
        encoded.len() > crate::bounds::MAX_RESPONSE_BYTES
    }) {
        json!({"data":{"truncated":true},"meta":{"truncated":true}})
    } else {
        body.clone()
    }
}

#[derive(Debug)]
pub struct MutationKey<'a> {
    pub session_id: i64,
    pub method: &'a str,
    pub path: &'a str,
    pub key: &'a str,
}

/// Hash a canonical JSON request body.
///
/// # Panics
/// Panics if serialization of an in-memory JSON value fails.
#[must_use]
pub fn body_hash(body: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(body).expect("JSON values serialize"))
    )
}

/// Reserve a request key or retrieve its completed response.
///
/// # Errors
/// Rejects missing or long keys, conflicting bodies, pending requests, and database errors.
pub fn reserve(
    ctx: Ctx<'_>,
    key: &MutationKey<'_>,
    body: &Value,
) -> Result<Reservation, MutationError> {
    if key.key.is_empty() || key.key.chars().count() > 128 {
        return Err(MutationError::InvalidKey);
    }
    let digest = body_hash(body);
    let now = ctx.clock.now_micros();
    let changed = ctx.conn.execute("INSERT OR IGNORE INTO idempotency_records(auth_session_id,method,normalized_route,key,request_hash,response_status,response_json,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,0,'{}',?6,?7)", params![key.session_id, key.method, key.path, key.key, digest, format_micros(now), format_micros(now.saturating_add(86_400_000_000))]).map_err(|_| MutationError::Database)?;
    if changed == 1 {
        return Ok(Reservation::New);
    }
    let row: Option<(String, i64, String)> = ctx.conn.query_row("SELECT request_hash,response_status,response_json FROM idempotency_records WHERE auth_session_id=?1 AND method=?2 AND normalized_route=?3 AND key=?4", params![key.session_id,key.method,key.path,key.key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|_| MutationError::Database)?;
    let (stored, status, body) = row.ok_or(MutationError::Database)?;
    if stored != digest {
        return Err(MutationError::Conflict);
    }
    if status == 0 {
        return Err(MutationError::InProgress);
    }
    Ok(Reservation::Replay {
        status: u16::try_from(status).map_err(|_| MutationError::Database)?,
        body: serde_json::from_str(&body).map_err(|_| MutationError::Database)?,
    })
}

/// Save a bounded response for a previously reserved request.
///
/// # Errors
/// The reservation is absent, already completed, or the database rejects the update.
pub fn store(
    ctx: Ctx<'_>,
    key: &MutationKey<'_>,
    status: u16,
    body: &Value,
) -> Result<(), MutationError> {
    if status == 0 {
        return Err(MutationError::Database);
    }
    let encoded =
        serde_json::to_string(&bounded_response(body)).map_err(|_| MutationError::Database)?;
    let changed = ctx.conn.execute("UPDATE idempotency_records SET response_status=?1,response_json=?2 WHERE auth_session_id=?3 AND method=?4 AND normalized_route=?5 AND key=?6 AND response_status=0", params![status, encoded, key.session_id,key.method,key.path,key.key]).map_err(|_| MutationError::Database)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(MutationError::Database)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atvprep_manager::{clock::FixedClock, db::Db, schema};
    #[test]
    fn closed_gate_response_includes_retry_after() {
        let response = MutationError::GateClosed.response("/api/v1/jobs");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(response.headers()[axum::http::header::RETRY_AFTER], "60");
    }
    #[test]
    fn pipeline_replays_once_and_rejects_conflicting_or_closed_requests() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("CREATE TABLE effects (id INTEGER); INSERT INTO deployment_gate(id,mode,phase,generation) VALUES(1,'open','steady',1) ON CONFLICT(id) DO UPDATE SET mode='open',phase='steady';").unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let auth = crate::auth::AuthService::with_csrf_ttl(ctx, 60, 20, 10).unwrap();
        let session = auth.create().unwrap();
        let csrf = auth
            .issue_csrf(&session.token, "POST", "/api/v1/jobs", "create")
            .unwrap();
        let body = json!({"action":"verify"});
        let request = MutationRequest {
            token: &session.token,
            csrf: &csrf,
            origin_allowed: true,
            requires_open_gate: true,
            method: "POST",
            path: "/api/v1/jobs",
            action: "create",
            idempotency_key: "key",
            body: &body,
            absolute_ttl: 60,
            idle_ttl: 20,
            csrf_ttl: 10,
        };
        let forbidden = MutationRequest {
            origin_allowed: false,
            ..request
        };
        assert_eq!(
            execute(ctx, &forbidden, |_| unreachable!()),
            Err(MutationError::Forbidden)
        );
        let run = || {
            execute(ctx, &request, |ctx| {
                ctx.conn
                    .execute("INSERT INTO effects VALUES(1)", [])
                    .unwrap();
                Ok((201, json!({"data":{"id":1}})))
            })
        };
        assert_eq!(run().unwrap().0, 201);
        let reused = MutationRequest {
            idempotency_key: "another",
            ..request
        };
        assert_eq!(
            execute(ctx, &reused, |_| unreachable!()),
            Err(MutationError::Csrf)
        );
        conn.raw()
            .execute(
                "UPDATE deployment_gate SET mode='maintenance' WHERE id=1",
                [],
            )
            .unwrap();
        assert_eq!(run().unwrap().0, 201);
        let count: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM effects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let different = json!({"action":"fix"});
        let conflicting = MutationRequest {
            body: &different,
            ..request
        };
        assert_eq!(
            execute(ctx, &conflicting, |_| unreachable!()),
            Err(MutationError::Conflict)
        );
        let fresh_csrf = auth
            .issue_csrf(&session.token, "POST", "/api/v1/jobs", "create")
            .unwrap();
        let closed = MutationRequest {
            csrf: &fresh_csrf,
            idempotency_key: "new",
            ..conflicting
        };
        assert_eq!(
            execute(ctx, &closed, |_| unreachable!()),
            Err(MutationError::GateClosed)
        );
        let cancel = MutationRequest {
            requires_open_gate: false,
            ..closed
        };
        assert!(execute(ctx, &cancel, |_| Ok((200, json!({})))).is_ok());
        conn.raw()
            .execute("UPDATE deployment_gate SET mode='open' WHERE id=1", [])
            .unwrap();
    }
    #[test]
    fn reserves_replays_and_rejects_conflicting_bodies() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw().execute("INSERT INTO auth_sessions(session_token_hash,csrf_secret_hash,expires_at) VALUES('a','b','2099-01-01')", []).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let key = MutationKey {
            session_id: 1,
            method: "POST",
            path: "/api/v1/jobs",
            key: "one",
        };
        let body = json!({"action":"verify"});
        assert_eq!(reserve(ctx, &key, &body), Ok(Reservation::New));
        assert_eq!(reserve(ctx, &key, &body), Err(MutationError::InProgress));
        assert_eq!(
            reserve(ctx, &key, &json!({"action":"fix"})),
            Err(MutationError::Conflict)
        );
        store(ctx, &key, 201, &json!({"data":{"id":1}})).unwrap();
        assert_eq!(
            reserve(ctx, &key, &body),
            Ok(Reservation::Replay {
                status: 201,
                body: json!({"data":{"id":1}})
            })
        );
    }

    #[test]
    fn simultaneous_reservations_have_one_winner() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("idempotency.db")).unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw().execute("INSERT INTO auth_sessions(session_token_hash,csrf_secret_hash,expires_at) VALUES('a','b','2099-01-01')", []).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let db = db.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let conn = db.conn().unwrap();
                    let clock = FixedClock(1_700_000_000_000_000);
                    let ctx = Ctx {
                        conn: conn.raw(),
                        clock: &clock,
                    };
                    let key = MutationKey {
                        session_id: 1,
                        method: "POST",
                        path: "/api/v1/jobs",
                        key: "race",
                    };
                    barrier.wait();
                    reserve(ctx, &key, &json!({"action":"verify"}))
                })
            })
            .collect();
        let outcomes: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == Ok(Reservation::New))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == Err(MutationError::InProgress))
                .count(),
            1
        );
    }
}

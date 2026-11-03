//! Browser control preserves local-operator authority, CSRF and replay rules.
use atvprep_api::{auth::AuthService, config::ManagerConfig, router, state::AppState};
use atvprep_manager::{
    db::{Conn, Db},
    deployment_gate::{DeploymentGate, Operator},
    repo::Ctx,
    schema,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;

fn fixture() -> (AppState, Conn, String) {
    fixture_with(Db::memory().unwrap())
}

fn fixture_with(db: Db) -> (AppState, Conn, String) {
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let state = AppState::new(
        db,
        ManagerConfig {
            api_secret: Some("secret".to_owned()),
            allowed_origins: vec!["http://testserver".to_owned()],
            ..ManagerConfig::default()
        },
    )
    .unwrap();
    let auth = AuthService::with_csrf_ttl(
        Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        },
        28800,
        3600,
        900,
    )
    .unwrap();
    let token = auth.create().unwrap().token;
    (state, conn, token)
}
fn csrf(state: &AppState, conn: &Conn, token: &str, path: &str, action: &str) -> String {
    AuthService::with_csrf_ttl(
        Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        },
        28800,
        3600,
        900,
    )
    .unwrap()
    .issue_csrf(token, "POST", path, action)
    .unwrap()
}
fn post(token: &str, csrf: &str, path: &str, body: &Value, key: &str) -> Request<Body> {
    Request::post(path)
        .header("origin", "http://testserver")
        .header("cookie", format!("__Host-atvprep_session={token}"))
        .header("x-csrf-token", csrf)
        .header("idempotency-key", key)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn browser_close_replay_and_generation_checked_open_work_during_maintenance() {
    let (state, conn, token) = fixture();
    let app = router(state.clone());
    let path = "/api/v1/deployment/close";
    let body = json!({"reason":"release check"});
    let close_proof = csrf(&state, &conn, &token, path, "close-gate");
    let response = app
        .clone()
        .oneshot(post(&token, &close_proof, path, &body, "close"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(first["data"]["mode"], "maintenance");
    let generation = first["data"]["generation"].as_i64().unwrap();
    let replay = app
        .clone()
        .oneshot(post(&token, &close_proof, path, &body, "close"))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let repeated: Value =
        serde_json::from_slice(&to_bytes(replay.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(first, repeated);
    assert_eq!(
        conn.raw()
            .query_row("SELECT count(*) FROM deployment_events", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    let path = "/api/v1/deployment/open";
    let bad = app
        .clone()
        .oneshot(post(
            &token,
            "invalid",
            path,
            &json!({"expected_generation":generation}),
            "bad",
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::FORBIDDEN);
    let proof = csrf(&state, &conn, &token, path, "open-gate");
    let outdated = app
        .clone()
        .oneshot(post(
            &token,
            &proof,
            path,
            &json!({"expected_generation":generation-1}),
            "stale",
        ))
        .await
        .unwrap();
    assert_eq!(outdated.status(), StatusCode::CONFLICT);
    let opened = app
        .oneshot(post(
            &token,
            &proof,
            path,
            &json!({"expected_generation":generation}),
            "open",
        ))
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
}
#[tokio::test]
async fn local_close_cannot_be_reopened_by_browser_and_missing_gate_is_reported_closed() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("manager.sqlite");
    let (state, conn, token) = fixture_with(Db::open(&database).unwrap());
    let gate = DeploymentGate(Ctx {
        conn: conn.raw(),
        clock: state.clock.as_ref(),
    });
    let operator = Operator::from_local_cli(&database).unwrap();
    let status = gate.close(&operator, "local release").unwrap();
    let path = "/api/v1/deployment/open";
    let proof = csrf(&state, &conn, &token, path, "open-gate");
    let response = router(state.clone())
        .oneshot(post(
            &token,
            &proof,
            path,
            &json!({"expected_generation":status.generation}),
            "local-open",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(
        body["detail"],
        "the gate was closed from the host; open it from the host"
    );

    conn.raw()
        .execute("DELETE FROM deployment_gate", [])
        .unwrap();
    let response = router(state)
        .oneshot(
            Request::get("/api/v1/deployment")
                .header("origin", "http://testserver")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["data"]["healthy"], false);
    assert_eq!(body["data"]["mode"], "maintenance");
}

#[tokio::test]
async fn unicode_reason_uses_character_limit_and_unprintable_notes_explain_the_conflict() {
    let (state, conn, token) = fixture();
    let path = "/api/v1/deployment/close";
    let proof = csrf(&state, &conn, &token, path, "close-gate");
    let reason = "ä".repeat(200);
    let response = router(state.clone())
        .oneshot(post(
            &token,
            &proof,
            path,
            &json!({"reason":reason}),
            "unicode-note",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["data"]["reason"], reason);
    let proof = csrf(&state, &conn, &token, path, "close-gate");
    let response = router(state)
        .oneshot(post(
            &token,
            &proof,
            path,
            &json!({"reason":"bad\treason"}),
            "unprintable-note",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(
        body["detail"],
        "the reason must be 1 to 200 printable characters"
    );
    let generation: i64 = conn
        .raw()
        .query_row(
            "SELECT generation FROM deployment_gate WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(generation, 1);
}

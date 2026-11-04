use atvprep_api::{auth::AuthService, config::ManagerConfig, router, state::AppState};
use atvprep_manager::{db::Db, repo::Ctx, schema};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn audit_pages_by_revision_and_filters_by_key() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch("INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original','true','bool',2),('max_probe_workers','4','int',3); INSERT INTO settings_audit(id,key,revision,actor,old_value_json,new_value_json) VALUES(1,'delete_original',2,'session:1','false','true'),(2,'max_probe_workers',2,'session:1','2','3'),(3,'max_probe_workers',3,'session:1','3','4');").unwrap();
    let config = ManagerConfig {
        api_secret: Some("secret".to_owned()),
        allowed_origins: vec!["http://testserver".to_owned()],
        ..ManagerConfig::default()
    };
    let state = AppState::new(db, config).unwrap();
    let auth = AuthService::with_csrf_ttl(
        Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        },
        28_800,
        3_600,
        900,
    )
    .unwrap();
    let token = auth.create().unwrap().token;
    let app = router(state);
    let get = |path: &str| {
        Request::get(path)
            .header("origin", "http://testserver")
            .header("cookie", format!("__Host-atvprep_session={token}"))
            .body(Body::empty())
            .unwrap()
    };
    let first = app
        .clone()
        .oneshot(get("/api/v1/settings/audit?key=max_probe_workers&limit=1"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value =
        serde_json::from_slice(&to_bytes(first.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(first["data"][0]["new_value"], 3);
    let cursor = first["meta"]["next_cursor"].as_str().unwrap();
    let second = app
        .oneshot(get(&format!(
            "/api/v1/settings/audit?key=max_probe_workers&limit=1&cursor={cursor}"
        )))
        .await
        .unwrap();
    let second: Value =
        serde_json::from_slice(&to_bytes(second.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(second["data"][0]["new_value"], 4);
    assert!(second["meta"]["next_cursor"].is_null());
}

//! A manual job with re-encode files needs an explicit confirmation (ADR 0007).
//! The server rejects it without `confirm_reencode`; the UI alone is not enough.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_manager::{db::Db, repo::Ctx, schema};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;

const SECRET: &str = "test-api-secret";

struct Harness {
    state: AppState,
    app: axum::Router,
    token: String,
    keys: std::cell::Cell<u32>,
}

impl Harness {
    fn new() -> Self {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw()
            .execute_batch(
                "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
            INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action,needs_reencode) VALUES
            (1,1,'plain.mp4','video','.mp4',1,1,'a','present','warning','idle','fix',0),
            (2,1,'old.mp4','video','.mp4',1,1,'b','present','warning','idle','fix',1);",
            )
            .unwrap();
        let state = AppState::new(
            db,
            ManagerConfig {
                api_secret: Some(SECRET.to_owned()),
                allowed_origins: vec!["http://testserver".to_owned()],
                ..ManagerConfig::default()
            },
        )
        .unwrap();
        let session = AuthService::new(
            Ctx {
                conn: conn.raw(),
                clock: state.clock.as_ref(),
            },
            28_800,
            3600,
        )
        .unwrap()
        .create()
        .unwrap();
        Self {
            app: router(state.clone()),
            state,
            token: session.token,
            keys: std::cell::Cell::new(0),
        }
    }

    fn sql<T: rusqlite::types::FromSql>(&self, sql: &str) -> T {
        let conn = self.state.db.conn().unwrap();
        conn.raw().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn id(media: i64) -> String {
        Opaque::new(SECRET.as_bytes()).encode("m", media)
    }

    async fn create_job(&self, body: Value) -> (StatusCode, Value) {
        let path = "/api/v1/jobs";
        let csrf = {
            let conn = self.state.db.conn().unwrap();
            AuthService::new(
                Ctx {
                    conn: conn.raw(),
                    clock: self.state.clock.as_ref(),
                },
                28_800,
                3600,
            )
            .unwrap()
            .issue_csrf(&self.token, "POST", path, "create-job")
            .unwrap()
        };
        self.keys.set(self.keys.get() + 1);
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("origin", "http://testserver")
            .header("cookie", format!("__Host-atvprep_session={}", self.token))
            .header("x-csrf-token", csrf)
            .header("idempotency-key", format!("key-{}", self.keys.get()))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn a_job_with_a_reencode_file_is_refused_without_confirmation() {
    let h = Harness::new();
    let ids = [Harness::id(1), Harness::id(2)];
    for action in ["prepare", "fix", "convert"] {
        let (status, body) = h
            .create_job(json!({"action":action,"media_ids":ids,"manual_approval":true}))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{action}: {body}");
        let (status, _) = h
            .create_job(json!({"action":action,"media_ids":ids,"manual_approval":true,"confirm_reencode":false}))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{action}");
    }
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 0);
}

#[tokio::test]
async fn a_confirmed_job_runs_and_records_the_confirmation() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1),Harness::id(2)],"manual_approval":true,"confirm_reencode":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM job_items"), 2);
    let snapshot: Value =
        serde_json::from_str(&h.sql::<String>("SELECT settings_snapshot_json FROM jobs")).unwrap();
    assert_eq!(snapshot["reencode_confirmed"], true);
}

#[tokio::test]
async fn files_without_reencode_need_no_confirmation() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let snapshot: Value =
        serde_json::from_str(&h.sql::<String>("SELECT settings_snapshot_json FROM jobs")).unwrap();
    assert_eq!(snapshot["reencode_confirmed"], false);
}

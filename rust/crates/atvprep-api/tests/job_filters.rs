//! The job list filters (status and action as OR lists, the search) and their facet counters.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_manager::{db::Db, repo::Ctx, schema};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Api {
    app: axum::Router,
    session: String,
}

impl Api {
    async fn call(&self, path: &str) -> (u16, Value) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("origin", "http://testserver")
                    .header("cookie", format!("__Host-atvprep_session={}", self.session))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body = serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap())
            .unwrap();
        (status, body)
    }

    async fn get(&self, path: &str) -> Value {
        let (status, body) = self.call(path).await;
        assert_eq!(status, 200, "{path}: {body}");
        body
    }

    /// The subjects of the jobs the list returns for a filter, in list order.
    async fn subjects(&self, query: &str) -> Vec<String> {
        let body = self.get(&format!("/api/v1/jobs?{query}")).await;
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|job| job["subject"].as_str().unwrap().to_owned())
            .collect()
    }
}

/// One job per job status, named after it by its only file; `prepare` is stored under its former
/// names. A cleared job never shows.
fn api() -> Api {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw()
        .execute_batch(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
             INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
               (1,1,'Films/Queued.mkv','video','.mkv',1,1,'f1','present','clean','idle','none'),
               (2,1,'Films/Running.mkv','video','.mkv',1,1,'f2','present','clean','idle','none'),
               (3,1,'Shows/Cancelling.mkv','video','.mkv',1,1,'f3','present','clean','idle','none'),
               (4,1,'Shows/Succeeded.mkv','video','.mkv',1,1,'f4','present','clean','idle','none'),
               (5,1,'Films/Partly.mkv','video','.mkv',1,1,'f5','present','clean','idle','none'),
               (6,1,'Films/Failed_1.mkv','video','.mkv',1,1,'f6','present','clean','idle','none'),
               (7,1,'Films/Cancelled.mkv','video','.mkv',1,1,'f7','present','clean','idle','none'),
               (8,1,'Films/Cleared.mkv','video','.mkv',1,1,'f8','present','clean','idle','none');
             INSERT INTO jobs(id,action,status) VALUES
               (1,'verify','queued'),(2,'fix','running'),(3,'inspect','cancel_requested'),(4,'verify','succeeded'),
               (5,'convert','partial_success'),(6,'fix','failed'),(7,'verify','cancelled'),(8,'verify','succeeded');
             INSERT INTO job_items(job_id,media_file_id,action,position,status) VALUES
               (1,1,'verify',0,'queued'),(2,2,'fix',0,'running'),(3,3,'inspect',0,'running'),(4,4,'verify',0,'succeeded'),
               (5,5,'convert',0,'failed'),(6,6,'fix',0,'failed'),(7,7,'verify',0,'cancelled'),(8,8,'verify',0,'succeeded');
             UPDATE jobs SET cleared_at='2026-01-01' WHERE id=8;",
        )
        .unwrap();
    let state = AppState::new(
        db,
        ManagerConfig {
            api_secret: Some("test-api-secret".to_owned()),
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
    .unwrap()
    .token;
    Api {
        app: router(state),
        session,
    }
}

const ALL_NEWEST_FIRST: [&str; 7] = [
    "Films/Cancelled.mkv",
    "Films/Failed_1.mkv",
    "Films/Partly.mkv",
    "Shows/Succeeded.mkv",
    "Shows/Cancelling.mkv",
    "Films/Running.mkv",
    "Films/Queued.mkv",
];

#[tokio::test]
async fn no_filter_lists_every_job_status_including_cancelled() {
    let api = api();
    assert_eq!(api.subjects("order=desc").await, ALL_NEWEST_FIRST);
    let mut oldest = ALL_NEWEST_FIRST;
    oldest.reverse();
    assert_eq!(api.subjects("order=asc").await, oldest);
}

#[tokio::test]
async fn status_takes_a_list_and_shows_jobs_in_any_of_them() {
    let api = api();
    assert_eq!(
        api.subjects("order=desc&status=failed,partial_success")
            .await,
        ["Films/Failed_1.mkv", "Films/Partly.mkv"]
    );
    assert_eq!(
        api.subjects("status=cancel_requested").await,
        ["Shows/Cancelling.mkv"]
    );
    // An empty list is no filter, as before.
    assert_eq!(api.subjects("order=desc&status=").await, ALL_NEWEST_FIRST);
}

#[tokio::test]
async fn action_takes_a_list_and_prepare_covers_the_former_names() {
    let api = api();
    assert_eq!(
        api.subjects("order=asc&action=prepare").await,
        [
            "Films/Running.mkv",
            "Films/Partly.mkv",
            "Films/Failed_1.mkv"
        ]
    );
    assert_eq!(
        api.subjects("order=asc&action=inspect,prepare&status=running,cancel_requested")
            .await,
        ["Films/Running.mkv", "Shows/Cancelling.mkv"]
    );
}

#[tokio::test]
async fn exclude_status_still_leaves_one_status_out() {
    let api = api();
    let shown = api.subjects("exclude_status=cancelled").await;
    assert_eq!(shown.len(), 6);
    assert!(!shown.contains(&"Films/Cancelled.mkv".to_owned()));
}

#[tokio::test]
async fn search_matches_the_subject_case_insensitively_and_literally() {
    let api = api();
    assert_eq!(
        api.subjects("order=asc&q=shows/").await,
        ["Shows/Cancelling.mkv", "Shows/Succeeded.mkv"]
    );
    // `_` and `%` are plain characters, not patterns.
    assert_eq!(api.subjects("q=d_1").await, ["Films/Failed_1.mkv"]);
    assert!(api.subjects("q=%25").await.is_empty());
}

#[tokio::test]
async fn unknown_tokens_and_long_values_are_refused() {
    let api = api();
    for path in [
        "/api/v1/jobs?status=bogus",
        "/api/v1/jobs?status=failed,bogus",
        "/api/v1/jobs?action=fix",
        "/api/v1/jobs?exclude_status=bogus",
        "/api/v1/jobs/counts?status=bogus",
        "/api/v1/jobs/counts?action=bogus",
    ] {
        assert_eq!(api.call(path).await.0, 422, "{path}");
    }
    let long = "a".repeat(129);
    for name in ["status", "action", "exclude_status", "q"] {
        let path = format!("/api/v1/jobs?{name}={long}");
        assert_eq!(api.call(&path).await.0, 422, "{path}");
        let path = format!("/api/v1/jobs/counts?{name}={long}");
        assert_eq!(api.call(&path).await.0, 422, "{path}");
    }
}

#[tokio::test]
async fn counts_vary_each_facet_with_the_other_filters() {
    let api = api();
    let body = api.get("/api/v1/jobs/counts").await;
    let movies = Opaque::new(b"test-api-secret").encode("r", 1);
    assert_eq!(
        body["data"],
        json!({
            "status": {"queued":1,"running":1,"cancel_requested":1,"succeeded":1,"partial_success":1,"failed":1,"cancelled":1},
            "action": {"verify":3,"prepare":3,"inspect":1},
            "root_id": {&movies: 7},
            "matched": 7, "total": 7, "hidden_active": 0
        })
    );

    // The status counts follow the action, the action counts the status; neither its own facet.
    let body = api
        .get("/api/v1/jobs/counts?status=failed,partial_success,succeeded&action=prepare")
        .await;
    assert_eq!(
        body["data"],
        json!({
            "status": {"queued":0,"running":1,"cancel_requested":0,"succeeded":0,"partial_success":1,"failed":1,"cancelled":0},
            "action": {"verify":1,"prepare":2,"inspect":0},
            "root_id": {&movies: 2},
            "matched": 2, "total": 7, "hidden_active": 3
        })
    );

    // The search narrows every count; the active jobs it leaves out are reported.
    let body = api.get("/api/v1/jobs/counts?q=shows").await;
    assert_eq!(body["data"]["status"]["cancel_requested"], 1);
    assert_eq!(body["data"]["status"]["queued"], 0);
    assert_eq!(body["data"]["matched"], 2);
    assert_eq!(body["data"]["hidden_active"], 2);
}

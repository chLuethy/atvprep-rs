//! The job list filters (status and action as OR lists, the search) and their facet counters.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_fixtures::{Fixtures, Verdict};
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
    api_with(|_| {})
}

/// The fixture of [`api`] plus what `extra` lays down after it, in the library of the files.
fn api_with(extra: impl FnOnce(&Fixtures<'_>)) -> Api {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    {
        let fx = Fixtures::new(conn.raw());
        let root = fx.library_at("Movies", "/library");
        let file = |name: &str| {
            fx.file(root, name)
                .fingerprint(&name.to_owned())
                .verdict(Verdict::Clean)
                .create()
        };
        let queued = file("Films/Queued.mkv");
        let running = file("Films/Running.mkv");
        let cancelling = file("Shows/Cancelling.mkv");
        let succeeded = file("Shows/Succeeded.mkv");
        let partly = file("Films/Partly.mkv");
        let failed = file("Films/Failed_1.mkv");
        let cancelled = file("Films/Cancelled.mkv");
        let cleared = file("Films/Cleared.mkv");
        let _ = fx.job("verify", &[queued]).create();
        let _ = fx.job("fix", &[running]).status("running").create();
        let _ = fx
            .job("inspect", &[cancelling])
            .status("cancel_requested")
            .item_status("running")
            .create();
        let _ = fx.job("verify", &[succeeded]).status("succeeded").create();
        let _ = fx
            .job("convert", &[partly])
            .status("partial_success")
            .item_status("failed")
            .create();
        let _ = fx.job("fix", &[failed]).status("failed").create();
        let _ = fx.job("verify", &[cancelled]).status("cancelled").create();
        let _ = fx
            .job("verify", &[cleared])
            .status("succeeded")
            .cleared_at("2026-01-01 00:00:00")
            .create();
        extra(&fx);
    }
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
async fn a_row_stored_as_prepare_is_listed_with_fix_and_convert() {
    // The stored form `prepare` is a historical row the repositories no longer write (ADR 0008), so
    // the one place that may lay it down is raw SQL.
    let api = api_with(|fx| {
        fx.sql("PRAGMA ignore_check_constraints=ON; UPDATE jobs SET action='prepare' WHERE id=6;");
    });
    let subjects = api.subjects("order=asc&action=prepare").await;
    assert_eq!(
        subjects,
        [
            "Films/Running.mkv",
            "Films/Partly.mkv",
            "Films/Failed_1.mkv"
        ]
    );
    let body = api.get("/api/v1/jobs?order=asc&action=prepare").await;
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|job| job["action"] == "prepare")
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

/// A job of three files whose search hits a later item, listed next to the fixture's jobs.
fn batch(fx: &Fixtures<'_>) {
    let root = 1;
    let files: Vec<i64> = [
        "Batch/First.mkv",
        "Batch/Hundredth Episode.mkv",
        "Batch/Hundredth Extra.mkv",
    ]
    .iter()
    .map(|name| {
        fx.file(root, name)
            .fingerprint(name)
            .verdict(Verdict::Clean)
            .create()
    })
    .collect();
    let _ = fx.job("verify", &files).status("succeeded").create();
}

#[tokio::test]
async fn search_finds_a_job_by_any_of_its_items_once() {
    let api = api_with(batch);
    let body = api.get("/api/v1/jobs?q=hundredth").await;
    let jobs = body["data"].as_array().unwrap();
    // Two items match, the job shows once; the row names the first matching item in job order.
    assert_eq!(jobs.len(), 1, "{body}");
    assert_eq!(jobs[0]["subject"], "Batch/First.mkv");
    assert_eq!(jobs[0]["matched_path"], "Batch/Hundredth Episode.mkv");

    // When the first file matches, the row needs no hint.
    let body = api.get("/api/v1/jobs?q=first").await;
    assert_eq!(body["data"][0]["subject"], "Batch/First.mkv");
    assert!(body["data"][0].get("matched_path").is_none(), "{body}");

    // The search combines with the other filters and narrows the counts alike.
    assert!(api.subjects("q=hundredth&status=failed").await.is_empty());
    assert_eq!(
        api.subjects("q=hundredth&action=verify&status=succeeded")
            .await,
        ["Batch/First.mkv"]
    );
    let counts = api.get("/api/v1/jobs/counts?q=hundredth").await;
    assert_eq!(counts["data"]["matched"], 1);
    assert_eq!(counts["data"]["status"]["succeeded"], 1);

    // Without a search no row carries the hint.
    let body = api.get("/api/v1/jobs").await;
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|job| job.get("matched_path").is_none())
    );
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
            "trigger": {"manual":0,"schedule":0,"retry":0}, "schedule_id": {}, "schedules": [],
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
            "trigger": {"manual":0,"schedule":0,"retry":0}, "schedule_id": {}, "schedules": [],
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

//! Orphaned partial rewrites: reported in the scan result and the overview, removed by hand only after
//! confirmation with count and size, and visible in the event history.
use std::{fs, path::Path};

use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_core::prepare::paths::TEMP_PREFIX;
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
    root: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw()
            .execute(
                "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies',?1)",
                [root.path().canonicalize().unwrap().to_str().unwrap()],
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
        let token = AuthService::new(
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
        Self {
            app: router(state.clone()),
            state,
            token,
            keys: std::cell::Cell::new(0),
            root,
        }
    }

    fn leftover(&self, dir: &str, stem: &str, bytes: usize) {
        let dir = self.root.path().join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{TEMP_PREFIX}enc.{stem}.4711-0a1b2c3d.mp4")),
            vec![b'x'; bytes],
        )
        .unwrap();
    }

    fn finished_scan(&self) -> i64 {
        let conn = self.state.db.conn().unwrap();
        conn.raw()
            .execute(
                "INSERT INTO scan_runs(root_id,generation,status)
                 SELECT 1, COALESCE(max(generation),0)+1, 'succeeded' FROM scan_runs WHERE root_id=1",
                [],
            )
            .unwrap();
        let id = conn.raw().last_insert_rowid();
        atvprep_manager::worker::orphans::report_for_scan(
            Ctx {
                conn: conn.raw(),
                clock: self.state.clock.as_ref(),
            },
            id,
            1,
        )
        .unwrap();
        id
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .uri(path)
            .header("origin", "http://testserver")
            .header("cookie", format!("__Host-atvprep_session={}", self.token))
            .body(Body::empty())
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn remove(&self, count: u64, bytes: u64) -> (StatusCode, Value) {
        let opaque = Opaque::new(SECRET.as_bytes());
        let path = format!("/api/v1/roots/{}/orphans/remove", opaque.encode("r", 1));
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
            .issue_csrf(&self.token, "POST", &path, "remove-orphans")
            .unwrap()
        };
        self.keys.set(self.keys.get() + 1);
        let request = Request::builder()
            .method("POST")
            .uri(&path)
            .header("origin", "http://testserver")
            .header("cookie", format!("__Host-atvprep_session={}", self.token))
            .header("x-csrf-token", csrf)
            .header("idempotency-key", format!("key-{}", self.keys.get()))
            .header("content-type", "application/json")
            .body(Body::from(json!({"count":count,"bytes":bytes}).to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    fn leftovers_on_disk(&self) -> usize {
        fn count(dir: &Path) -> usize {
            fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap())
                .map(|e| {
                    if e.file_type().unwrap().is_dir() {
                        count(&e.path())
                    } else {
                        usize::from(e.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
                    }
                })
                .sum()
        }
        count(self.root.path())
    }
}

#[tokio::test]
async fn the_scan_result_names_count_size_and_list_and_says_nothing_without_hits() {
    let harness = Harness::new();
    let scan = harness.finished_scan();
    let opaque = Opaque::new(SECRET.as_bytes());
    let path = format!("/api/v1/scans/{}", opaque.encode("s", scan));
    let (status, body) = harness.get(&path).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].get("orphans").is_none(), "{body}");

    harness.leftover("Show", "Film", 5);
    harness.leftover("Show", "Other", 2);
    let scan = harness.finished_scan();
    let path = format!("/api/v1/scans/{}", opaque.encode("s", scan));
    let (_, body) = harness.get(&path).await;
    let orphans = &body["data"]["orphans"];
    assert_eq!(orphans["count"], 2);
    assert_eq!(orphans["bytes"], 7);
    assert_eq!(orphans["files"].as_array().unwrap().len(), 2);
    assert!(
        orphans["files"][0]["relative_path"]
            .as_str()
            .unwrap()
            .starts_with(&format!("Show/{TEMP_PREFIX}enc."))
    );
    assert_eq!(orphans["files"][0]["size"], 5);
    assert!(orphans["files"][0]["modified_at"].is_string());
}

#[tokio::test]
async fn the_overview_lists_hits_of_the_latest_scan_only_when_there_are_any() {
    let harness = Harness::new();
    harness.finished_scan();
    let (_, body) = harness.get("/api/v1/dashboard").await;
    assert!(body["data"].get("orphans").is_none(), "{body}");

    harness.leftover("Show", "Film", 5);
    harness.finished_scan();
    let (_, body) = harness.get("/api/v1/dashboard").await;
    let hits = body["data"]["orphans"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["name"], "Movies");
    assert_eq!(hits[0]["count"], 1);
    assert_eq!(hits[0]["bytes"], 5);
    assert!(hits[0]["root_id"].as_str().unwrap().starts_with("r_"));
}

#[tokio::test]
async fn confirmed_removal_deletes_the_files_updates_the_report_and_shows_in_the_history() {
    let harness = Harness::new();
    harness.leftover("Show", "Film", 5);
    harness.leftover("Other", "Film", 2);
    fs::write(harness.root.path().join("Show/Film.mkv"), "source").unwrap();
    harness.finished_scan();

    let (status, body) = harness.remove(2, 7).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["removed"], 2);
    assert_eq!(body["data"]["bytes"], 7);
    assert_eq!(harness.leftovers_on_disk(), 0);
    assert!(harness.root.path().join("Show/Film.mkv").exists());
    let (_, dashboard) = harness.get("/api/v1/dashboard").await;
    assert!(dashboard["data"].get("orphans").is_none());
    let (_, events) = harness.get("/api/v1/events").await;
    let topics: Vec<&str> = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["topic"].as_str().unwrap())
        .collect();
    assert_eq!(
        topics.iter().filter(|t| **t == "orphan.removed").count(),
        2,
        "{topics:?}"
    );
    assert!(topics.contains(&"orphan.cleanup"));
    let removed = events["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["topic"] == "orphan.removed")
        .unwrap();
    assert!(removed["job_id"].is_null());
    assert!(removed["entity_id"].as_str().unwrap().starts_with("r_"));
    let (_, activity) = harness.get("/api/v1/dashboard/activity").await;
    assert!(
        activity["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["topic"] == "orphan.cleanup"),
        "{activity}"
    );
}

#[tokio::test]
async fn a_wrong_confirmation_deletes_nothing_and_returns_the_current_numbers() {
    let harness = Harness::new();
    harness.leftover("Show", "Film", 5);
    harness.finished_scan();

    let (status, body) = harness.remove(3, 99).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["data"]["count"], 1);
    assert_eq!(body["data"]["bytes"], 5);
    assert_eq!(harness.leftovers_on_disk(), 1);
}

#[tokio::test]
async fn removal_needs_a_session_and_a_known_library() {
    let harness = Harness::new();
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/roots/r_bogus/orphans/remove")
        .header("origin", "http://testserver")
        .header("content-type", "application/json")
        .header("idempotency-key", "k")
        .body(Body::from(json!({"count":0,"bytes":0}).to_string()))
        .unwrap();
    let response = harness.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

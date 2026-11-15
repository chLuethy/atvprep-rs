//! Every item of a Vorbereiten job keeps the expected change it was queued with: the Änderungsart
//! and the planned steps the preview showed at that moment (`planned_change`, `planned_steps`).
//! Older items and the items of Prüfen and Analysieren keep their shape.
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
const SAMPLE_RATE: &str =
    "[warning] audio-sample-rate: ac-3 track id=2 sample rate 44100Hz (expected 48000)";
const FAST_START: &str = "[warning] fast-start: moov atom is not at the front";

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
        let reasons = serde_json::to_string(&[SAMPLE_RATE, FAST_START]).unwrap();
        conn.raw()
            .execute(
                "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library')",
                [],
            )
            .unwrap();
        conn.raw()
            .execute(
                "INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,compliance_reasons,workflow_state,desired_action,encode_need,planned_change) VALUES
                (1,1,'film.mp4','video','.mp4',1,1,'a','present','warning',?1,'idle','fix','audio','audio'),
                (2,1,'clean.mp4','video','.mp4',1,1,'b','present','clean','[]','idle','none','none','none')",
                [reasons],
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

    fn exec(&self, sql: &str) {
        let conn = self.state.db.conn().unwrap();
        conn.raw().execute_batch(sql).unwrap();
    }

    fn media(id: i64) -> String {
        Opaque::new(SECRET.as_bytes()).encode("m", id)
    }

    async fn queue(&self, body: Value) -> String {
        let (status, created) = self.post("/api/v1/jobs", "create-job", body).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created["data"]["id"].as_str().unwrap().to_owned()
    }

    async fn items(&self, job: &str) -> Vec<Value> {
        let body = self.get(&format!("/api/v1/jobs/{job}/items")).await;
        body["data"].as_array().unwrap().clone()
    }

    async fn get(&self, path: &str) -> Value {
        let request = Request::builder()
            .uri(path)
            .header("origin", "http://testserver")
            .header("cookie", format!("__Host-atvprep_session={}", self.token))
            .body(Body::empty())
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn post(&self, path: &str, action: &str, body: Value) -> (StatusCode, Value) {
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
            .issue_csrf(&self.token, "POST", path, action)
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

fn prepare(media: &[i64]) -> Value {
    let ids: Vec<String> = media.iter().map(|id| Harness::media(*id)).collect();
    json!({"action":"prepare","media_ids":ids,"manual_approval":true,"encode":"audio"})
}

#[tokio::test]
async fn a_queued_prepare_item_reports_the_change_and_steps_of_its_file() {
    let h = Harness::new();
    let job = h.queue(prepare(&[1])).await;
    let items = h.items(&job).await;
    assert_eq!(items[0]["planned_change"], "audio", "{items:?}");
    assert_eq!(
        items[0]["planned_steps"],
        json!([SAMPLE_RATE, FAST_START]),
        "{items:?}"
    );
}

#[tokio::test]
async fn the_item_keeps_the_planned_values_after_the_file_changed() {
    let h = Harness::new();
    let job = h.queue(prepare(&[1])).await;
    h.exec(
        "UPDATE job_items SET status='succeeded';
         UPDATE media_files SET compliance_state='clean',compliance_reasons='[]',encode_need='none',planned_change='none' WHERE id=1;",
    );
    let items = h.items(&job).await;
    assert_eq!(items[0]["planned_change"], "audio", "{items:?}");
    assert_eq!(items[0]["planned_steps"], json!([SAMPLE_RATE, FAST_START]));
}

#[tokio::test]
async fn a_file_with_nothing_listed_has_an_empty_step_list() {
    let h = Harness::new();
    let job = h.queue(prepare(&[2])).await;
    let items = h.items(&job).await;
    assert_eq!(items[0]["planned_change"], "none", "{items:?}");
    assert_eq!(items[0]["planned_steps"], json!([]));
}

#[tokio::test]
async fn older_prepare_items_and_other_actions_have_no_planned_fields() {
    let h = Harness::new();
    h.exec(
        "INSERT INTO jobs(id,action,status,trigger) VALUES(1,'fix','succeeded','manual');
         INSERT INTO job_items(id,job_id,media_file_id,action,position,status,settings_snapshot_json)
           VALUES(1,1,1,'fix',0,'succeeded','{\"pipeline\":\"prepare\",\"policy\":\"encode-v3\",\"encode\":\"audio\"}');",
    );
    let mut jobs = vec![Opaque::new(SECRET.as_bytes()).encode("j", 1)];
    for action in ["verify", "inspect"] {
        jobs.push(
            h.queue(
                json!({"action":action,"media_ids":[Harness::media(1)],"manual_approval":true}),
            )
            .await,
        );
    }
    for job in jobs {
        let items = h.items(&job).await;
        assert!(items[0].get("planned_change").is_none(), "{items:?}");
        assert!(items[0].get("planned_steps").is_none(), "{items:?}");
    }
}

#[tokio::test]
async fn the_steps_are_redacted_and_bounded() {
    let h = Harness::new();
    let long = format!("[warning] apple-tags: {}", "x".repeat(4000));
    let reasons = serde_json::to_string(&[
        "[warning] fast-start: token=abc C:\\private\\film.mp4".to_owned(),
        long,
    ])
    .unwrap();
    h.exec(&format!(
        "UPDATE media_files SET compliance_reasons='{reasons}' WHERE id=1"
    ));
    let job = h.queue(prepare(&[1])).await;
    let items = h.items(&job).await;
    let steps = items[0]["planned_steps"].as_array().unwrap();
    let first = steps[0].as_str().unwrap();
    assert!(
        !first.contains("abc") && !first.contains("private"),
        "{first}"
    );
    assert!(steps[1].as_str().unwrap().len() <= 512, "{}", steps[1]);
}

#[tokio::test]
async fn at_most_32_steps_are_kept() {
    let h = Harness::new();
    let many: Vec<String> = (0..50)
        .map(|n| format!("[warning] apple-tags: finding {n}"))
        .collect();
    h.exec(&format!(
        "UPDATE media_files SET compliance_reasons='{}' WHERE id=1",
        serde_json::to_string(&many).unwrap()
    ));
    let job = h.queue(prepare(&[1])).await;
    let items = h.items(&job).await;
    assert_eq!(items[0]["planned_steps"].as_array().unwrap().len(), 32);
}

#[tokio::test]
async fn subtitle_files_still_to_merge_are_one_step() {
    let h = Harness::new();
    h.exec(
        "INSERT INTO sidecars(id,media_file_id,relative_path,size,mtime_ns,fingerprint) VALUES
           (1,1,'film.de.srt',1,1,'s1'),(2,1,'film.en.forced.srt',1,1,'s2');
         INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result)
           VALUES(1,'a','a','ffprobe','1','{\"streams\":[]}');",
    );
    let job = h.queue(prepare(&[1])).await;
    let items = h.items(&job).await;
    assert_eq!(
        items[0]["planned_steps"],
        json!([SAMPLE_RATE, FAST_START, "[embed] subtitles: 2"]),
        "{items:?}"
    );
}

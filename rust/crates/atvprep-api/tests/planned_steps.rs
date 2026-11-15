//! Every item of a Vorbereiten job keeps the expected change it was queued with: the Änderungsart
//! and the planned steps the preview showed at that moment (`planned_change`, `planned_steps`).
//! Older items and the items of Prüfen and Analysieren keep their shape.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_fixtures::Fixtures;
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

/// A verify report of `film.mp4`: the findings (rule, message), the Kodierbedarf and Änderungsart and
/// the steps the planner names.
fn report(findings: &[(&str, &str)], need: &str, change: &str, steps: &[Value]) -> Value {
    let violations: Vec<Value> = findings
        .iter()
        .map(|(rule, message)| json!({"severity": "warning", "rule": rule, "message": message}))
        .collect();
    json!({"violations": violations, "encode_need": need, "planned_change": change,
        "planned_steps": steps})
}

/// The step the planner stored for the file, as the API shows it.
fn rate_step() -> Value {
    json!({"kind": "sample_rate", "stream": 2, "from_codec": "ac3", "to_codec": "eac3",
        "from_hz": 44100, "to_hz": 48000})
}

/// A step known only as text.
fn text(text: &str) -> Value {
    json!({"kind": "text", "text": text})
}

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
        {
            let fx = Fixtures::new(conn.raw());
            let root = fx.library_at("Movies", "/library");
            let film = fx
                .file(root, "film.mp4")
                .fingerprint("a")
                .desired_action("fix")
                .create();
            let _ = fx.inspection(
                film,
                "verify",
                "a",
                &report(
                    &[
                        (
                            "audio-sample-rate",
                            "ac-3 track id=2 sample rate 44100Hz (expected 48000)",
                        ),
                        ("fast-start", "moov atom is not at the front"),
                    ],
                    "audio",
                    "audio",
                    &[rate_step()],
                ),
            );
            // Never checked: no findings, no steps.
            let _ = fx.file(root, "clean.mp4").fingerprint("b").create();
        }
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

    /// A newer verify report of `film.mp4`; it replaces what the last one projected.
    fn recheck(&self, findings: &[(&str, &str)], need: &str, change: &str, steps: &[Value]) {
        let conn = self.state.db.conn().unwrap();
        let _ = Fixtures::new(conn.raw()).inspection(
            1,
            "verify",
            "a",
            &report(findings, need, change, steps),
        );
    }

    /// Raw SQL for what no repository lays down: the status of an item (the worker's state machine
    /// owns it) and the stored form of results and items from before steps were data.
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
        json!([text(SAMPLE_RATE), text(FAST_START), rate_step()]),
        "{items:?}"
    );
}

#[tokio::test]
async fn the_item_keeps_the_planned_values_after_the_file_changed() {
    let h = Harness::new();
    let job = h.queue(prepare(&[1])).await;
    h.exec("UPDATE job_items SET status='succeeded'");
    h.recheck(&[], "none", "none", &[]);
    let items = h.items(&job).await;
    assert_eq!(items[0]["planned_change"], "audio", "{items:?}");
    assert_eq!(
        items[0]["planned_steps"],
        json!([text(SAMPLE_RATE), text(FAST_START), rate_step()])
    );
}

#[tokio::test]
async fn a_file_with_nothing_listed_has_an_empty_step_list() {
    let h = Harness::new();
    let job = h.queue(prepare(&[2])).await;
    let items = h.items(&job).await;
    // A file nobody checked yet: the change is unknown and nothing is listed.
    assert_eq!(items[0]["planned_change"], "unknown", "{items:?}");
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
    let long = "x".repeat(4000);
    h.recheck(
        &[
            ("fast-start", "token=abc C:\\private\\film.mp4"),
            ("apple-tags", &long),
        ],
        "audio",
        "audio",
        &[],
    );
    let job = h.queue(prepare(&[1])).await;
    let items = h.items(&job).await;
    let steps = items[0]["planned_steps"].as_array().unwrap();
    // The findings are listed in the order of their rules: apple-tags, then fast-start.
    let first = steps[1]["text"].as_str().unwrap();
    assert!(
        !first.contains("abc") && !first.contains("private"),
        "{first}"
    );
    assert!(
        steps[0]["text"].as_str().unwrap().len() <= 512,
        "{}",
        steps[0]
    );
}

#[tokio::test]
async fn at_most_32_steps_are_kept() {
    let h = Harness::new();
    let many: Vec<String> = (0..50).map(|n| format!("finding {n}")).collect();
    let findings: Vec<(&str, &str)> = many.iter().map(|m| ("apple-tags", m.as_str())).collect();
    h.recheck(&findings, "audio", "audio", &[]);
    let job = h.queue(prepare(&[1])).await;
    let items = h.items(&job).await;
    assert_eq!(items[0]["planned_steps"].as_array().unwrap().len(), 32);
}

#[tokio::test]
async fn subtitle_files_still_to_merge_are_one_step() {
    let h = Harness::new();
    {
        let conn = h.state.db.conn().unwrap();
        let fx = Fixtures::new(conn.raw());
        let _ = fx.sidecar(1, "film.de.srt", "s1");
        let _ = fx.sidecar(1, "film.en.forced.srt", "s2");
        let _ = fx.inspection(1, "ffprobe", "a", &json!({"streams": []}));
    }
    let job = h.queue(prepare(&[1])).await;
    let items = h.items(&job).await;
    assert_eq!(
        items[0]["planned_steps"],
        json!([
            text(SAMPLE_RATE),
            text(FAST_START),
            rate_step(),
            text("[embed] subtitles: 2")
        ]),
        "{items:?}"
    );
}

const OLD_PLAN: &str = "audio stream #1 ac3 44100Hz -> eac3 48000Hz";

/// What a check before steps were data stored: the plan as a `[plan]` reason.
fn store_old_plan(h: &Harness) {
    let reasons = serde_json::to_string(&[FAST_START, &format!("[plan] {OLD_PLAN}")]).unwrap();
    h.exec(&format!(
        "UPDATE media_files SET compliance_reasons='{reasons}', planned_steps='[]' WHERE id=1"
    ));
}

#[tokio::test]
async fn a_plan_stored_as_text_by_an_older_check_is_a_text_step() {
    let h = Harness::new();
    store_old_plan(&h);
    let job = h.queue(prepare(&[1])).await;
    let items = h.items(&job).await;
    assert_eq!(
        items[0]["planned_steps"],
        json!([text(FAST_START), text(OLD_PLAN)]),
        "{items:?}"
    );
}

#[tokio::test]
async fn an_item_of_an_older_job_shows_its_text_steps_as_text() {
    let h = Harness::new();
    h.exec(
        "INSERT INTO jobs(id,action,status,trigger) VALUES(1,'fix','succeeded','manual');
         INSERT INTO job_items(id,job_id,media_file_id,action,position,status,settings_snapshot_json)
           VALUES(1,1,1,'fix',0,'succeeded','{\"pipeline\":\"prepare\",\"policy\":\"encode-v3\",\"encode\":\"audio\",\"profile\":{\"crf\":18,\"fix_dts\":false,\"keep_av1\":false,\"prefer_hevc10\":false},\"manual_approval\":true,\"approved_by\":1,\"planned_change\":\"audio\",\"planned_steps\":[\"[warning] fast-start: x\",\"[plan] audio stream #1 ac3 44100Hz -> eac3 48000Hz\"]}');",
    );
    let items = h
        .items(&Opaque::new(SECRET.as_bytes()).encode("j", 1))
        .await;
    assert_eq!(
        items[0]["planned_steps"],
        json!([text("[warning] fast-start: x"), text(OLD_PLAN)]),
        "{items:?}"
    );
}

#[tokio::test]
async fn the_file_detail_lists_the_steps_apart_from_the_reasons() {
    let h = Harness::new();
    let detail = h.get(&format!("/api/v1/files/{}", Harness::media(1))).await;
    assert_eq!(
        detail["data"]["compliance_reasons"],
        json!([SAMPLE_RATE, FAST_START])
    );
    assert_eq!(detail["data"]["planned_steps"], json!([rate_step()]));
    let clean = h.get(&format!("/api/v1/files/{}", Harness::media(2))).await;
    assert_eq!(clean["data"]["planned_steps"], json!([]));
}

#[tokio::test]
async fn the_file_detail_reads_an_older_plan_reason_as_a_text_step() {
    let h = Harness::new();
    store_old_plan(&h);
    let detail = h.get(&format!("/api/v1/files/{}", Harness::media(1))).await;
    assert_eq!(detail["data"]["compliance_reasons"], json!([FAST_START]));
    assert_eq!(detail["data"]["planned_steps"], json!([text(OLD_PLAN)]));
}

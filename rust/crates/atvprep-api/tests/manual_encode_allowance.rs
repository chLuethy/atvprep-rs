//! A manual job carries a Kodierfreigabe; the video level needs an explicit confirmation (ADR 0008).
//! The server rejects it without `confirm_video_encode`; the UI alone is not enough.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_core::encode::EncodeAllowance::{self, All, Audio, Copy};
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
            INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action,encode_need) VALUES
            (1,1,'plain.mp4','video','.mp4',1,1,'a','present','warning','idle','fix','unknown'),
            (2,1,'old.mp4','video','.mp4',1,1,'b','present','warning','idle','fix','video');",
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
        self.post("/api/v1/jobs", "create-job", body).await
    }

    /// A failed run of the newest job's only item, so it can be retried.
    fn fail_the_item(&self) {
        let conn = self.state.db.conn().unwrap();
        conn.raw()
            .execute(
                "UPDATE job_items SET status='failed' WHERE job_id=(SELECT max(id) FROM jobs)",
                [],
            )
            .unwrap();
    }

    /// Mark the newest job's only item as refused and return its opaque id.
    fn refuse_the_item(&self) -> String {
        let conn = self.state.db.conn().unwrap();
        conn.raw()
            .execute(
                "UPDATE job_items SET status='succeeded',error_code='encode_not_allowed',error_detail='Encoding not allowed (need=video allowance=audio): the file stays as it was.' WHERE job_id=(SELECT max(id) FROM jobs)",
                [],
            )
            .unwrap();
        Opaque::new(SECRET.as_bytes()).encode("i", self.sql::<i64>("SELECT max(id) FROM job_items"))
    }

    async fn raise(&self, item: &str, body: Value) -> (StatusCode, Value) {
        self.post(
            &format!("/api/v1/job-items/{item}/raise-encode"),
            "raise-encode",
            body,
        )
        .await
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
        self.send("POST", path, action, body).await
    }

    async fn send(
        &self,
        method: &'static str,
        path: &str,
        action: &str,
        body: Value,
    ) -> (StatusCode, Value) {
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
            .issue_csrf(&self.token, method, path, action)
            .unwrap()
        };
        self.keys.set(self.keys.get() + 1);
        let request = Request::builder()
            .method(method)
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
async fn audio_and_video_is_refused_without_the_confirmation() {
    let h = Harness::new();
    let ids = [Harness::id(1), Harness::id(2)];
    for action in ["prepare"] {
        let (status, body) = h
            .create_job(
                json!({"action":action,"media_ids":ids,"manual_approval":true,"encode":"all"}),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{action}: {body}");
        let (status, _) = h
            .create_job(json!({"action":action,"media_ids":ids,"manual_approval":true,"encode":"all","confirm_video_encode":false}))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{action}");
    }
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 0);
}

#[tokio::test]
async fn audio_and_video_with_the_confirmation_is_recorded_on_every_item() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1),Harness::id(2)],"manual_approval":true,"encode":"all","confirm_video_encode":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let snapshot: Value =
        serde_json::from_str(&h.sql::<String>("SELECT settings_snapshot_json FROM jobs")).unwrap();
    assert_eq!(snapshot["policy"], "encode-v3");
    assert_eq!(snapshot["video_encode_confirmed"], true);
    assert_eq!(
        item_decisions(&h),
        [(1, All, false), (2, All, false)],
        "the level is the operator's, whatever the files need"
    );
}

#[tokio::test]
async fn audio_is_the_default_and_needs_no_confirmation_even_for_a_file_that_needs_video() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(2)],"manual_approval":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(item_decisions(&h), [(2, Audio, false)]);
}

#[tokio::test]
async fn copy_is_recorded_as_chosen() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true,"encode":"copy"}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(item_decisions(&h), [(1, Copy, false)]);
}

#[tokio::test]
async fn an_unknown_level_is_refused() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true,"encode":"video","confirm_video_encode":true}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 0);
}

#[tokio::test]
async fn the_delete_original_setting_is_recorded_and_signed_with_the_job() {
    let h = Harness::new();
    {
        let conn = h.state.db.conn().unwrap();
        conn.raw()
            .execute(
                "INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original','true','bool',1)",
                [],
            )
            .unwrap();
    }
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(2)],"manual_approval":true,"encode":"all","confirm_video_encode":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(item_decisions(&h), [(2, All, true)]);
}

fn newest_force(h: &Harness) -> bool {
    h.sql::<String>("SELECT settings_snapshot_json FROM job_items ORDER BY id DESC LIMIT 1")
        .contains("\"force\":true")
}

#[tokio::test]
async fn force_is_recorded_and_signed_on_a_manual_prepare() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true,"encode":"copy","force":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(item_decisions(&h), [(1, Copy, false)]);
    assert!(newest_force(&h));
}

#[tokio::test]
async fn force_is_off_unless_asked_for() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(item_decisions(&h), [(1, Audio, false)]);
    assert!(!newest_force(&h));
}

#[tokio::test]
async fn force_is_refused_for_an_action_that_changes_nothing() {
    let h = Harness::new();
    for action in ["inspect", "verify"] {
        let (status, body) = h
            .create_job(json!({"action":action,"media_ids":[Harness::id(1)],"force":true}))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{action}: {body}");
    }
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 0);
}

/// The Zielprofil recorded on the newest item.
fn newest_profile(h: &Harness) -> Value {
    let text: String = h.sql("SELECT settings_snapshot_json FROM job_items ORDER BY id DESC");
    serde_json::from_str::<Value>(&text).unwrap()["profile"].clone()
}

async fn set_settings(h: &Harness, settings: Value) -> (StatusCode, Value) {
    h.send(
        "PATCH",
        "/api/v1/settings",
        "settings",
        json!({"settings":settings}),
    )
    .await
}

#[tokio::test]
async fn a_prepare_records_and_signs_the_current_target_profile() {
    let h = Harness::new();
    let (status, body) = set_settings(&h, json!({"target_crf":24,"target_keep_av1":true})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(2)],"manual_approval":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        newest_profile(&h),
        json!({"crf":24,"prefer_hevc10":false,"keep_av1":true,"fix_dts":false})
    );
    item_decisions(&h);
}

#[tokio::test]
async fn the_settings_list_the_target_profile_at_its_defaults() {
    let h = Harness::new();
    let request = Request::builder()
        .uri("/api/v1/settings")
        .header("origin", "http://testserver")
        .header("cookie", format!("__Host-atvprep_session={}", h.token))
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let entry = |key: &str| {
        body["data"]["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|setting| setting["key"] == key)
            .unwrap_or_else(|| panic!("{key} is listed"))
            .clone()
    };
    assert_eq!(entry("target_crf")["value"], json!(18));
    assert_eq!(entry("target_crf")["default_value"], json!(18));
    for key in ["target_prefer_hevc10", "target_keep_av1", "target_fix_dts"] {
        assert_eq!(entry(key)["value"], json!(false), "{key}");
    }
}

#[tokio::test]
async fn the_server_refuses_an_invalid_target_profile() {
    let h = Harness::new();
    for settings in [
        json!({"target_crf":52}),
        json!({"target_crf":-1}),
        json!({"target_crf":"18"}),
        json!({"target_keep_av1":1}),
        json!({"target_prefer_hevc10":"yes"}),
        json!({"target_fix_dts":null}),
    ] {
        let (status, body) = set_settings(&h, settings.clone()).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{settings} {body}"
        );
    }
    assert_eq!(
        h.sql::<i64>("SELECT count(*) FROM runtime_settings WHERE key LIKE 'target_%'"),
        0
    );
    assert_eq!(
        h.sql::<String>("SELECT encode_need FROM media_files WHERE id=2"),
        "video",
        "a refused change leaves the needs alone"
    );
}

#[tokio::test]
async fn a_changed_target_profile_makes_the_needs_unknown_through_the_api() {
    let h = Harness::new();
    let (status, body) = set_settings(&h, json!({"target_keep_av1":true})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        h.sql::<i64>("SELECT count(*) FROM media_files WHERE encode_need<>'unknown'"),
        0
    );
}

#[tokio::test]
async fn a_retry_after_a_profile_change_is_refused_and_says_to_queue_again() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(2)],"manual_approval":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    h.fail_the_item();
    let (status, body) = set_settings(&h, json!({"target_crf":24})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let job = body_job(&h);
    let (status, body) = h
        .post(
            &format!("/api/v1/jobs/{job}/retry"),
            "retry-job",
            json!({"job_id":job,"operation":"retry"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("queue it again"), "{body}");
}

/// The opaque id of the newest job.
fn body_job(h: &Harness) -> String {
    Opaque::new(SECRET.as_bytes()).encode("j", h.sql::<i64>("SELECT max(id) FROM jobs"))
}

#[tokio::test]
async fn a_retry_takes_the_stored_allowance_and_force_unchanged() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(2)],"manual_approval":true,"encode":"audio","force":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    h.fail_the_item();
    let job = body["data"]["id"].as_str().unwrap();
    let (status, body) = h
        .post(
            &format!("/api/v1/jobs/{job}/retry"),
            "retry-job",
            json!({"job_id":job,"operation":"retry"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(item_decisions(&h), [(2, Audio, false)]);
    assert!(newest_force(&h));
}

#[tokio::test]
async fn a_refused_item_is_raised_as_a_new_signed_job_with_its_own_confirmation() {
    let h = Harness::new();
    let (status, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(2)],"manual_approval":true,"force":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let original: i64 = h.sql("SELECT max(id) FROM jobs");
    let item = h.refuse_the_item();

    let (status, body) = h.raise(&item, json!({"encode":"all"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 1);

    let (status, body) = h
        .raise(&item, json!({"encode":"all","confirm_video_encode":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body["data"]["id"].is_string());
    assert!(h.sql::<i64>("SELECT max(id) FROM jobs") > original);
    assert_eq!(
        h.sql::<String>("SELECT trigger FROM jobs ORDER BY id DESC LIMIT 1"),
        "manual"
    );
    assert_eq!(item_decisions(&h), [(2, All, false)]);
    assert!(
        !newest_force(&h),
        "the higher release decides only the level"
    );
}

#[tokio::test]
async fn a_refused_copy_item_can_be_raised_to_audio_without_a_confirmation() {
    let h = Harness::new();
    let (_, body) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true,"encode":"copy"}))
        .await;
    assert!(body["data"]["id"].is_string());
    let item = h.refuse_the_item();
    let (status, body) = h.raise(&item, json!({"encode":"audio"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(item_decisions(&h), [(1, Audio, false)]);
}

#[tokio::test]
async fn the_items_of_a_job_report_their_allowance() {
    let h = Harness::new();
    let (_, created) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true,"encode":"copy"}))
        .await;
    let job = created["data"]["id"].as_str().unwrap();
    let request = Request::builder()
        .uri(format!("/api/v1/jobs/{job}/items"))
        .header("origin", "http://testserver")
        .header("cookie", format!("__Host-atvprep_session={}", h.token))
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"][0]["encode"], "copy", "{body}");
}

#[tokio::test]
async fn the_items_of_a_job_report_the_stored_need_of_their_file() {
    let h = Harness::new();
    let (_, created) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1),Harness::id(2)],"manual_approval":true,"encode":"copy"}))
        .await;
    let job = created["data"]["id"].as_str().unwrap();
    let body = h.get(&format!("/api/v1/jobs/{job}/items")).await;
    assert_eq!(body["data"][0]["encode_need"], "unknown", "{body}");
    assert_eq!(body["data"][1]["encode_need"], "video", "{body}");
}

#[tokio::test]
async fn raising_needs_a_higher_level_than_the_item_had() {
    let h = Harness::new();
    h.create_job(json!({"action":"prepare","media_ids":[Harness::id(2)],"manual_approval":true,"encode":"audio"}))
        .await;
    let item = h.refuse_the_item();
    for level in ["copy", "audio"] {
        let (status, body) = h.raise(&item, json!({"encode":level})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{level}: {body}");
    }
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 1);
}

#[tokio::test]
async fn only_an_item_with_encoding_not_allowed_can_be_raised() {
    let h = Harness::new();
    h.create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true,"encode":"copy"}))
        .await;
    h.fail_the_item();
    let item =
        Opaque::new(SECRET.as_bytes()).encode("i", h.sql::<i64>("SELECT max(id) FROM job_items"));
    let (status, body) = h.raise(&item, json!({"encode":"audio"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 1);
}

/// The Kodierfreigabe recorded on each item, by media id, with its signature checked.
#[allow(clippy::too_many_lines)] // One walk through everything the signature binds.
fn item_decisions(h: &Harness) -> Vec<(i64, EncodeAllowance, bool)> {
    use atvprep_manager::{
        approval::{ApprovalItem, ApprovalVerifier, item_encode, prepare_policy_block},
        jobs::Jobs,
        snapshots::InputSnapshots,
    };
    let conn = h.state.db.conn().unwrap();
    let ctx = Ctx {
        conn: conn.raw(),
        clock: h.state.clock.as_ref(),
    };
    let verifier = ApprovalVerifier::new(h.state.approval_secret.as_bytes()).unwrap();
    let job: i64 = h.sql("SELECT max(id) FROM jobs");
    Jobs(ctx)
        .items(job)
        .unwrap()
        .iter()
        .map(|item| {
            let recorded = InputSnapshots(ctx).get_for_item(item.id).unwrap().unwrap();
            let identity = [
                recorded["file_device"].clone(),
                recorded["file_inode"].clone(),
                recorded["file_ctime_ns"].clone(),
                recorded["sidecar_fingerprint"].clone(),
            ];
            let encode = item_encode(&item.settings_snapshot).expect("every item has a level");
            let delete_original = item.settings_snapshot["delete_original"] == json!(true);
            assert_eq!(prepare_policy_block(&item.settings_snapshot), None);
            let signed = |encode, delete_original| {
                verifier.check(
                    &item.settings_snapshot,
                    &ApprovalItem {
                        job_id: job,
                        media_id: item.media_file_id,
                        action: &item.action,
                        identity: &identity,
                        encode,
                        delete_original,
                        force: atvprep_manager::approval::item_force(&item.settings_snapshot),
                        profile: atvprep_manager::approval::item_profile(&item.settings_snapshot)
                            .unwrap(),
                    },
                    ctx.clock.now_micros() / 1_000_000,
                )
            };
            assert_eq!(signed(encode, delete_original), None);
            for other in [Copy, Audio, All].into_iter().filter(|e| *e != encode) {
                assert!(
                    signed(other, delete_original).is_some(),
                    "the level is part of the signature"
                );
            }
            assert!(signed(encode, !delete_original).is_some());
            let force = atvprep_manager::approval::item_force(&item.settings_snapshot);
            assert!(
                verifier
                    .check(
                        &item.settings_snapshot,
                        &ApprovalItem {
                            job_id: job,
                            media_id: item.media_file_id,
                            action: &item.action,
                            identity: &identity,
                            encode,
                            delete_original,
                            force: !force,
                            profile: atvprep_manager::approval::item_profile(
                                &item.settings_snapshot
                            )
                            .unwrap(),
                        },
                        ctx.clock.now_micros() / 1_000_000,
                    )
                    .is_some(),
                "the decision to rebuild is part of the signature"
            );
            assert!(
                verifier
                    .check(
                        &item.settings_snapshot,
                        &ApprovalItem {
                            job_id: job,
                            media_id: item.media_file_id,
                            action: &item.action,
                            identity: &identity,
                            encode,
                            delete_original,
                            force,
                            profile: atvprep_manager::settings::TargetProfile {
                                crf: 40,
                                ..atvprep_manager::approval::item_profile(&item.settings_snapshot)
                                    .unwrap()
                            },
                        },
                        ctx.clock.now_micros() / 1_000_000,
                    )
                    .is_some(),
                "the target profile is part of the signature"
            );
            (item.media_file_id, encode, delete_original)
        })
        .collect()
}

#[tokio::test]
async fn the_old_action_names_are_no_longer_accepted() {
    let h = Harness::new();
    for action in ["fix", "convert"] {
        let (status, body) = h
            .create_job(
                json!({"action":action,"media_ids":[Harness::id(1)],"manual_approval":true}),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{action}: {body}");
    }
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 0);
}

#[tokio::test]
async fn a_prepare_is_answered_and_listed_as_prepare() {
    let h = Harness::new();
    let (status, created) = h
        .create_job(json!({"action":"prepare","media_ids":[Harness::id(1)],"manual_approval":true}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["data"]["action"], "prepare");
    let job = created["data"]["id"].as_str().unwrap();
    assert_eq!(
        h.get(&format!("/api/v1/jobs/{job}")).await["data"]["action"],
        "prepare"
    );
    assert_eq!(
        h.get("/api/v1/jobs?action=prepare").await["data"][0]["id"],
        job
    );
}

#[tokio::test]
async fn existing_history_entries_read_as_prepare() {
    let h = Harness::new();
    {
        let conn = h.state.db.conn().unwrap();
        conn.raw()
            .execute_batch(
                "INSERT INTO jobs(id,action,status,trigger) VALUES(1,'fix','succeeded','manual'),(2,'convert','failed','manual');
                 INSERT INTO job_items(id,job_id,media_file_id,action,position,status) VALUES(1,1,1,'fix',0,'succeeded'),(2,2,2,'convert',0,'failed');",
            )
            .unwrap();
    }
    let listed = h.get("/api/v1/jobs").await;
    let actions: Vec<&str> = listed["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["prepare", "prepare"], "{listed}");
    let queue = h.get("/api/v1/dashboard/queue?limit=5").await;
    assert!(!queue.to_string().contains("\"fix\""), "{queue}");
    let by_filter = h.get("/api/v1/jobs?action=prepare").await;
    assert_eq!(
        by_filter["data"].as_array().unwrap().len(),
        2,
        "{by_filter}"
    );
    assert!(
        h.get("/api/v1/jobs?action=fix").await["data"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for (job, item) in [(1, 1), (2, 2)] {
        let opaque = Opaque::new(SECRET.as_bytes());
        let job = opaque.encode("j", job);
        assert_eq!(
            h.get(&format!("/api/v1/jobs/{job}")).await["data"]["action"],
            "prepare"
        );
        let items = h.get(&format!("/api/v1/jobs/{job}/items")).await;
        assert_eq!(items["data"][0]["action"], "prepare", "{items}");
        assert_eq!(items["data"][0]["id"], opaque.encode("i", item));
    }
}

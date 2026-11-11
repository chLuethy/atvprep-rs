//! Schedules that change files: created paused, turned on only with an explicit confirmation that
//! signs the schedule, and the signature lapses when its parameters change or the schedule pauses.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_manager::{
    approval::{ApprovalVerifier, ScheduleParams},
    db::Db,
    repo::Ctx,
    schema,
};
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
    session_id: i64,
    keys: std::cell::Cell<u32>,
}

impl Harness {
    fn new() -> Self {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
            INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
            (1,1,'one.mp4','video','.mp4',1,1,'one','present','clean','idle','none'),
            (2,1,'two.mp4','video','.mp4',1,1,'two','present','warning','idle','none');
            UPDATE media_files SET encode_need='none';").unwrap();
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
            session_id: session.id,
            keys: std::cell::Cell::new(0),
        }
    }

    fn sql<T: rusqlite::types::FromSql>(&self, sql: &str) -> T {
        let conn = self.state.db.conn().unwrap();
        conn.raw().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    async fn send(
        &self,
        method: &str,
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

    async fn create(&self, body: Value) -> (StatusCode, Value) {
        self.send("POST", "/api/v1/schedules", "create-schedule", body)
            .await
    }

    async fn patch(&self, id: &str, body: Value) -> (StatusCode, Value) {
        self.send(
            "PATCH",
            &format!("/api/v1/schedules/{id}"),
            "update-schedule",
            body,
        )
        .await
    }

    /// The stored approval verifies against the schedule as it is now.
    fn approval_holds(&self, id: i64) -> bool {
        let conn = self.state.db.conn().unwrap();
        let row = atvprep_manager::schedules::ScheduleApproval::load(conn.raw(), id)
            .unwrap()
            .unwrap();
        let delete_original =
            atvprep_manager::schedules::delete_original_setting(conn.raw()).unwrap();
        let profile = atvprep_manager::settings::target_profile(conn.raw()).unwrap();
        let params: ScheduleParams<'_> = row.params(delete_original, profile);
        ApprovalVerifier::new(self.state.approval_secret.as_bytes())
            .unwrap()
            .check_schedule(
                &params,
                row.approved_by,
                row.approved_at,
                row.approval_mac.as_deref(),
            )
            .is_none()
    }
}

fn new_schedule(action: &str) -> Value {
    json!({"name":"Nightly","action":action,"days":[0],"time":"03:00","max_files":5})
}

#[tokio::test]
async fn the_old_actions_are_refused_and_preparing_is_the_one_that_changes_files() {
    let h = Harness::new();
    for action in ["fix", "convert"] {
        assert_eq!(
            h.create(new_schedule(action)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{action}"
        );
    }
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM schedules"), 0);
}

#[tokio::test]
async fn preparing_schedules_start_paused_and_need_max_files() {
    let h = Harness::new();
    for action in ["prepare"] {
        let (status, body) = h.create(new_schedule(action)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["data"]["action"], action);
        assert_eq!(body["data"]["enabled"], false);
        assert_eq!(body["data"]["max_files"], 5);
        for bad in [json!(0), json!(501), json!(-1)] {
            let mut input = new_schedule(action);
            input["max_files"] = bad;
            assert_eq!(
                h.create(input).await.0,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{action}"
            );
        }
        let mut input = new_schedule(action);
        input.as_object_mut().unwrap().remove("max_files");
        assert_eq!(h.create(input).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM schedules"), 1);
    assert_eq!(
        h.sql::<i64>("SELECT count(*) FROM schedules WHERE enabled=1"),
        0
    );
    assert_eq!(
        h.sql::<i64>("SELECT count(*) FROM schedules WHERE approval_mac IS NOT NULL"),
        0
    );
    // Checking still defaults max_files and starts on.
    let (status, body) = h
        .create(json!({"name":"Check","action":"verify","days":[0],"time":"03:00"}))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["enabled"], true);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One schedule walks through every way an approval is given and lapses.
async fn turning_on_needs_confirmation_and_every_change_of_parameters_or_pause_revokes_it() {
    let h = Harness::new();
    let (_, body) = h.create(new_schedule("prepare")).await;
    let token = body["data"]["id"].as_str().unwrap().to_owned();
    let id = Opaque::new(SECRET.as_bytes()).decode(&token, "sc").unwrap();

    // Without the explicit confirmation the schedule stays paused and unsigned.
    for body in [
        json!({"enabled":true}),
        json!({"enabled":true,"confirm_changes_files":false}),
    ] {
        let (status, _) = h.patch(&token, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(h.sql::<i64>("SELECT enabled FROM schedules"), 0);
    assert!(!h.approval_holds(id));

    // Confirmed: on, and signed by the signed-in operator for exactly these parameters.
    let (status, body) = h
        .patch(&token, json!({"enabled":true,"confirm_changes_files":true}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["enabled"], true);
    assert!(h.approval_holds(id));
    assert_eq!(
        h.sql::<i64>("SELECT approved_by FROM schedules"),
        h.session_id
    );

    // A name or time is not a parameter of the approval.
    let (status, _) = h
        .patch(&token, json!({"name":"Renamed","days":[1,2]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(h.approval_holds(id));
    assert_eq!(h.sql::<i64>("SELECT enabled FROM schedules"), 1);

    // Every parameter change pauses the schedule and revokes the approval.
    for change in [
        json!({"max_files":11}),
        json!({"compliance_state":"warning"}),
        json!({"clear_compliance_state":true}),
        json!({"root_id": Opaque::new(SECRET.as_bytes()).encode("r", 1)}),
        json!({"clear_root":true}),
        json!({"encode":"audio"}),
        json!({"encode":"copy"}),
    ] {
        let (status, _) = h
            .patch(&token, json!({"enabled":true,"confirm_changes_files":true}))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(h.approval_holds(id), "before {change}");
        let (status, body) = h.patch(&token, change.clone()).await;
        assert_eq!(status, StatusCode::OK, "{change}");
        assert_eq!(body["data"]["enabled"], false, "{change}");
        assert!(!h.approval_holds(id), "{change}");
        assert_eq!(
            h.sql::<Option<String>>("SELECT approval_mac FROM schedules"),
            None,
            "{change}"
        );
    }

    // Changing a parameter and turning on in one confirmed request signs the new parameters.
    let (status, _) = h
        .patch(
            &token,
            json!({"max_files":12,"enabled":true,"confirm_changes_files":true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(h.approval_holds(id));
    assert_eq!(h.sql::<i64>("SELECT max_files FROM schedules"), 12);

    // Pausing revokes; turning on again needs a new confirmation.
    let (status, _) = h.patch(&token, json!({"enabled":false})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!h.approval_holds(id));
    assert_eq!(
        h.sql::<Option<String>>("SELECT approval_mac FROM schedules"),
        None
    );
    assert_eq!(
        h.patch(&token, json!({"enabled":true})).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // The delete_original setting and the Zielprofil are part of what was signed.
    for setting in [
        "INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original','true','bool',1)",
        "INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('target_crf','20','int',1)",
        "INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('target_keep_av1','true','bool',1)",
    ] {
        let (status, _) = h
            .patch(&token, json!({"enabled":true,"confirm_changes_files":true}))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(h.approval_holds(id));
        let conn = h.state.db.conn().unwrap();
        conn.raw().execute(setting, []).unwrap();
        assert!(!h.approval_holds(id), "{setting}");
        conn.raw()
            .execute(
                "DELETE FROM runtime_settings WHERE key LIKE 'target_%' OR key='delete_original'",
                [],
            )
            .unwrap();
    }
}

#[tokio::test]
async fn the_allowance_starts_at_copy_is_signed_and_shown_with_the_profile() {
    let h = Harness::new();
    // Created at Nur kopieren, and only schedules that change files have one.
    let (status, body) = h.create(new_schedule("prepare")).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["encode"], "copy");
    assert_eq!(
        body["data"]["profile"],
        json!({"crf":18,"prefer_hevc10":false,"keep_av1":false,"fix_dts":false})
    );
    let token = body["data"]["id"].as_str().unwrap().to_owned();
    let id = Opaque::new(SECRET.as_bytes()).decode(&token, "sc").unwrap();
    let (_, body) = h
        .create(json!({"name":"Check","action":"verify","days":[0],"time":"03:00"}))
        .await;
    assert!(body["data"]["encode"].is_null(), "{body}");
    assert!(body["data"]["profile"].is_null(), "{body}");
    for encode in ["audio", "all"] {
        let mut input = new_schedule("prepare");
        input["encode"] = json!(encode);
        let (status, body) = h.create(input).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["data"]["encode"], encode);
    }
    for bad in [json!("video"), json!(true), json!("")] {
        let mut input = new_schedule("prepare");
        input["encode"] = bad.clone();
        assert_eq!(
            h.create(input).await.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}"
        );
        assert_eq!(
            h.patch(&token, json!({"encode": bad})).await.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}"
        );
    }
    let mut input = json!({"name":"Check","action":"verify","days":[0],"time":"03:00"});
    input["encode"] = json!("audio");
    assert_eq!(h.create(input).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    // Turning on together with an allowance signs it.
    let (status, body) = h
        .patch(
            &token,
            json!({"encode":"all","enabled":true,"confirm_changes_files":true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["encode"], "all");
    assert_eq!(body["data"]["enabled"], true);
    assert!(h.approval_holds(id));
    // Lowering it pauses and revokes.
    let (status, body) = h.patch(&token, json!({"encode":"audio"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["encode"], "audio");
    assert_eq!(body["data"]["enabled"], false);
    assert!(!h.approval_holds(id));
    // Saving the same value is not a change.
    let (status, _) = h
        .patch(&token, json!({"enabled":true,"confirm_changes_files":true}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = h.patch(&token, json!({"encode":"audio"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], true);
    assert!(h.approval_holds(id));
}

#[tokio::test]
async fn the_server_caps_files_per_run_at_twelve_without_video_and_six_with_it() {
    let h = Harness::new();
    for (encode, ok, over) in [("copy", 12, 13), ("audio", 12, 13), ("all", 6, 7)] {
        let mut input = new_schedule("prepare");
        input["encode"] = json!(encode);
        input["max_files"] = json!(over);
        let (status, body) = h.create(input.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{encode}");
        assert!(
            body["detail"].as_str().unwrap().contains(&ok.to_string()),
            "{body}"
        );
        input["max_files"] = json!(ok);
        assert_eq!(h.create(input).await.0, StatusCode::CREATED, "{encode}");
    }
    // Raising the allowance of a schedule with 12 files per run is refused, also while pausing.
    let (_, body) = h.create(new_schedule("prepare")).await;
    let token = body["data"]["id"].as_str().unwrap().to_owned();
    let (status, _) = h.patch(&token, json!({"max_files":12})).await;
    assert_eq!(status, StatusCode::OK);
    for change in [
        json!({"encode":"all"}),
        json!({"encode":"all","enabled":false}),
    ] {
        let (status, body) = h.patch(&token, change.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{change}");
        assert!(body["detail"].as_str().unwrap().contains('6'), "{body}");
    }
    let (status, _) = h.patch(&token, json!({"encode":"all","max_files":6})).await;
    assert_eq!(status, StatusCode::OK);
    // The list publishes both limits.
    let request = axum::http::Request::get("/api/v1/schedules")
        .header("origin", "http://testserver")
        .header("cookie", format!("__Host-atvprep_session={}", h.token))
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    let listed: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(
        listed["meta"]["limits"]["max_files"],
        json!({"verify":500,"prepare":12})
    );
    assert_eq!(listed["meta"]["limits"]["max_files_video"], 6);
}

#[tokio::test]
async fn matching_counts_the_files_whose_need_lies_within_the_allowance() {
    let h = Harness::new();
    let (_, body) = h.create(new_schedule("prepare")).await;
    let token = body["data"]["id"].as_str().unwrap().to_owned();
    h.state
        .db
        .conn()
        .unwrap()
        .raw()
        .execute_batch(
            "INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action,encode_need)
             VALUES(3,1,'a.mkv','video','.mkv',1,1,'a','present','warning','idle','none','audio'),
                   (4,1,'b.mkv','video','.mkv',1,1,'b','present','warning','idle','none','video'),
                   (5,1,'c.mkv','video','.mkv',1,1,'c','present','warning','idle','none','unknown');
             INSERT OR REPLACE INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original','true','bool',1);",
        )
        .unwrap();
    // two.mp4 needs nothing; a needs audio, b video and c is not known yet.
    for (encode, matching) in [("copy", 1), ("audio", 2), ("all", 4)] {
        let (_, body) = h.patch(&token, json!({"encode":encode})).await;
        assert_eq!(body["data"]["matching"], matching, "{encode}");
    }
}

#[tokio::test]
async fn run_now_queues_only_an_approved_change_schedule() {
    let h = Harness::new();
    let (_, body) = h.create(new_schedule("prepare")).await;

    let token = body["data"]["id"].as_str().unwrap().to_owned();
    let run = format!("/api/v1/schedules/{token}/run");

    let (status, body) = h
        .send("POST", &run, "run-schedule", json!({"schedule_id":token}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["data"]["last_result"]
            .as_str()
            .unwrap()
            .starts_with("Not queued")
    );
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM jobs"), 0);

    let (status, _) = h
        .patch(&token, json!({"enabled":true,"confirm_changes_files":true}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = h
        .send("POST", &run, "run-schedule", json!({"schedule_id":token}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["last_result"], "Queued 1 file");
    assert_eq!(h.sql::<String>("SELECT action FROM jobs"), "fix");
    let snapshot: Value =
        serde_json::from_str(&h.sql::<String>("SELECT settings_snapshot_json FROM jobs")).unwrap();
    assert_eq!(snapshot["schedule_params"]["max_files"], 5);
}

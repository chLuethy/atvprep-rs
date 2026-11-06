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
            (2,1,'two.mp4','video','.mp4',1,1,'two','present','warning','idle','none');").unwrap();
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
        let params: ScheduleParams<'_> = row.params(delete_original);
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
async fn fix_and_convert_schedules_start_paused_and_need_max_files() {
    let h = Harness::new();
    for action in ["fix", "convert"] {
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
    assert_eq!(h.sql::<i64>("SELECT count(*) FROM schedules"), 2);
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
    let (_, body) = h.create(new_schedule("fix")).await;
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

    // The delete_original setting is part of what was signed.
    let (status, _) = h
        .patch(&token, json!({"enabled":true,"confirm_changes_files":true}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(h.approval_holds(id));
    h.state
        .db
        .conn()
        .unwrap()
        .raw()
        .execute("INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('delete_original','true','bool',1)", [])
        .unwrap();
    assert!(!h.approval_holds(id));
}

#[tokio::test]
async fn run_now_queues_only_an_approved_change_schedule() {
    let h = Harness::new();
    let (_, body) = h.create(new_schedule("fix")).await;
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

//! The Auslöser of a job (by hand, from a schedule, retry), derived by the server, as a filter
//! with facet counters, per schedule too, and in the job data.
use atvprep_api::{auth::AuthService, config::ManagerConfig, router, state::AppState};
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

    /// The subjects of the jobs the list returns for a filter, oldest first.
    async fn subjects(&self, query: &str) -> Vec<String> {
        let body = self.get(&format!("/api/v1/jobs?order=asc&{query}")).await;
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|job| job["subject"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The job whose only file is `subject`, as the list delivers it.
    async fn job(&self, subject: &str) -> Value {
        let body = self.get("/api/v1/jobs?order=asc").await;
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["subject"] == subject)
            .unwrap()
            .clone()
    }

    /// The opaque id of a schedule, as `/jobs/counts` lists it with its name.
    async fn schedule(&self, name: &str) -> String {
        let body = self.get("/api/v1/jobs/counts").await;
        body["data"]["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|schedule| schedule["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

/// Jobs from every Auslöser, named after it by their only file. Schedules store their jobs as
/// `manual`; a run of the schedule points at the job. "Orphan" came from a schedule since deleted,
/// whose runs went with it; only the job's snapshot still names it. The retry of a scheduled job
/// keeps that job's snapshot but is a retry.
fn api() -> Api {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw()
        .execute_batch(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
             INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
               (1,1,'Hand.mkv','video','.mkv',1,1,'f1','present','clean','idle','none'),
               (2,1,'Night.mkv','video','.mkv',1,1,'f2','present','clean','idle','none'),
               (3,1,'Weekend.mkv','video','.mkv',1,1,'f3','present','clean','idle','none'),
               (4,1,'Retry.mkv','video','.mkv',1,1,'f4','present','clean','idle','none'),
               (5,1,'Orphan.mkv','video','.mkv',1,1,'f5','present','clean','idle','none'),
               (6,1,'Night2.mkv','video','.mkv',1,1,'f6','present','clean','idle','none');
             INSERT INTO schedules(id,name,action,days_mask,time_of_day) VALUES
               (1,'Nachtlauf','verify',127,'02:00'),(2,'Wochenende','prepare',65,'03:00'),(3,'Scan','scan',127,'01:00');
             INSERT INTO jobs(id,action,status,trigger,origin_job_id,settings_snapshot_json) VALUES
               (1,'verify','succeeded','manual',NULL,'{}'),
               (2,'verify','failed','manual',NULL,'{\"schedule_id\":1}'),
               (3,'fix','succeeded','manual',NULL,'{\"schedule_id\":2}'),
               (4,'verify','succeeded','retry',2,'{\"schedule_id\":1}'),
               (5,'verify','succeeded','manual',NULL,'{\"schedule_id\":9}'),
               (6,'verify','succeeded','manual',NULL,'not json');
             INSERT INTO job_items(job_id,media_file_id,action,position,status) VALUES
               (1,1,'verify',0,'succeeded'),(2,2,'verify',0,'failed'),(3,3,'fix',0,'succeeded'),
               (4,4,'verify',0,'succeeded'),(5,5,'verify',0,'succeeded'),(6,6,'verify',0,'succeeded');
             INSERT INTO schedule_runs(schedule_id,slot,started_at,trigger,result,job_id) VALUES
               (1,'2026-11-01 02:00','2026-11-01 02:00','scheduled','Queued 1 file',2),
               (2,NULL,'2026-11-01 03:00','manual','Queued 1 file',3),
               (1,'2026-11-02 02:00','2026-11-02 02:00','scheduled','Queued 1 file',6),
               (3,'2026-11-01 01:00','2026-11-01 01:00','scheduled','Queued 1 scan',NULL);",
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

#[tokio::test]
async fn trigger_filters_by_the_derived_ausloeser() {
    let api = api();
    // A job from a schedule is stored as `manual` but never counts as by hand.
    assert_eq!(api.subjects("trigger=manual").await, ["Hand.mkv"]);
    assert_eq!(
        api.subjects("trigger=schedule").await,
        ["Night.mkv", "Weekend.mkv", "Orphan.mkv", "Night2.mkv"]
    );
    assert_eq!(api.subjects("trigger=retry").await, ["Retry.mkv"]);
    assert_eq!(
        api.subjects("trigger=retry,manual").await,
        ["Hand.mkv", "Retry.mkv"]
    );
    assert_eq!(api.subjects("trigger=").await.len(), 6);
}

#[tokio::test]
async fn schedule_filters_by_single_schedules_and_adds_to_the_trigger() {
    let api = api();
    let night = api.schedule("Nachtlauf").await;
    let weekend = api.schedule("Wochenende").await;
    assert_eq!(
        api.subjects(&format!("schedule={night}")).await,
        ["Night.mkv", "Night2.mkv"]
    );
    assert_eq!(
        api.subjects(&format!("schedule={night},{weekend}")).await,
        ["Night.mkv", "Weekend.mkv", "Night2.mkv"]
    );
    // Within the facet Auslöser the choices are alternatives.
    assert_eq!(
        api.subjects(&format!("trigger=manual&schedule={weekend}"))
            .await,
        ["Hand.mkv", "Weekend.mkv"]
    );
    // The other facets still narrow.
    assert_eq!(
        api.subjects(&format!("schedule={night}&status=failed"))
            .await,
        ["Night.mkv"]
    );
}

#[tokio::test]
async fn counts_per_trigger_and_per_schedule_follow_the_other_filters() {
    let api = api();
    let night = api.schedule("Nachtlauf").await;
    let weekend = api.schedule("Wochenende").await;
    let body = api.get("/api/v1/jobs/counts").await;
    let data = &body["data"];
    assert_eq!(data["trigger"], json!({"manual":1,"schedule":4,"retry":1}));
    assert_eq!(
        data["schedule"],
        json!({night.clone():2, weekend.clone():1})
    );
    // Schedules that queue jobs, by name; a scan schedule never does.
    assert_eq!(
        data["schedules"],
        json!([{"id":night,"name":"Nachtlauf"},{"id":weekend,"name":"Wochenende"}])
    );
    assert_eq!(data["matched"], 6);

    // The Auslöser counts ignore their own choice but follow the status; the status counts follow the
    // Auslöser.
    let body = api
        .get(&format!(
            "/api/v1/jobs/counts?trigger=manual&schedule={night}&status=succeeded"
        ))
        .await;
    let data = &body["data"];
    assert_eq!(data["trigger"], json!({"manual":1,"schedule":3,"retry":1}));
    assert_eq!(data["schedule"], json!({night.clone():1, weekend:1}));
    assert_eq!(data["status"]["succeeded"], 2);
    assert_eq!(data["status"]["failed"], 1);
    assert_eq!(data["matched"], 2);
}

#[tokio::test]
async fn job_data_names_the_derived_trigger_and_the_schedule() {
    let api = api();
    let night = api.schedule("Nachtlauf").await;
    let job = api.job("Hand.mkv").await;
    assert_eq!(job["trigger"], "manual");
    assert!(job.get("schedule").is_none());
    let job = api.job("Night.mkv").await;
    assert_eq!(job["trigger"], "schedule");
    assert_eq!(job["schedule"], json!({"id":night,"name":"Nachtlauf"}));
    let job = api.job("Retry.mkv").await;
    assert_eq!(job["trigger"], "retry");
    assert!(job.get("schedule").is_none());
    // A deleted schedule has no name any more.
    let job = api.job("Orphan.mkv").await;
    assert_eq!(job["trigger"], "schedule");
    assert_eq!(job["schedule"]["name"], Value::Null);
    // The single job reads the same.
    let id = api.job("Night.mkv").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = api.get(&format!("/api/v1/jobs/{id}")).await;
    assert_eq!(body["data"]["trigger"], "schedule");
    assert_eq!(body["data"]["schedule"]["name"], "Nachtlauf");
}

#[tokio::test]
async fn unknown_triggers_and_schedules_are_refused() {
    let api = api();
    let job = api.job("Hand.mkv").await["id"].as_str().unwrap().to_owned();
    for query in [
        "trigger=automatic".to_owned(),
        "trigger=manual,bogus".to_owned(),
        "schedule=bogus".to_owned(),
        // A job id is no schedule id.
        format!("schedule={job}"),
        format!("trigger={}", "a".repeat(129)),
    ] {
        for list in ["jobs", "jobs/counts"] {
            let path = format!("/api/v1/{list}?{query}");
            assert_eq!(api.call(&path).await.0, 422, "{path}");
        }
    }
}

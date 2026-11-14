#[path = "support/seed.rs"]
mod seed;
// Differential checks against responses recorded from the Python manager.

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

const SECRET: &str = "test-api-secret";

#[test]
fn python_schema_lists_the_read_contract_routes() {
    let schema: Value =
        serde_json::from_str(include_str!("../../../testdata/api/openapi.json")).unwrap();
    let paths = schema["paths"].as_object().unwrap();
    for path in [
        "/api/v1/dashboard",
        "/api/v1/dashboard/summary",
        "/api/v1/dashboard/activity",
        "/api/v1/dashboard/queue",
        "/api/v1/files",
        "/api/v1/browse",
        "/api/v1/browse/below",
        "/api/v1/roots",
        "/api/v1/scans",
        "/api/v1/jobs",
        "/api/v1/jobs/counts",
        "/api/v1/events",
        "/api/v1/settings",
        "/api/v1/schedules",
    ] {
        assert!(
            paths.get(path).and_then(|item| item.get("get")).is_some(),
            "missing GET {path}"
        );
    }
}

fn normalise(value: &mut Value, key: &str, opaque: &Opaque) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                normalise(value, key, opaque);
            }
        }
        Value::Array(items) => {
            for value in items {
                normalise(value, "", opaque);
            }
        }
        Value::String(text) if key.ends_with("_at") => "<ts>".clone_into(text),
        Value::String(text) => {
            if let Some((kind, _)) = text.split_once('_')
                && matches!(
                    kind,
                    "r" | "s" | "j" | "i" | "m" | "e" | "c" | "n" | "sc" | "cursor:events"
                )
                && let Ok(id) = opaque.decode(text, kind)
            {
                *text = format!("<{kind}:{id}>");
            }
        }
        _ => {}
    }
}

fn fixture_path(name: &str, job: &str, item: &str, media: &str, schedule: &str) -> String {
    match name {
        "roots" => "/api/v1/roots".to_owned(),
        "files" => "/api/v1/files".to_owned(),
        "browse" => "/api/v1/browse".to_owned(),
        "dismiss_missing" => "/api/v1/files/dismiss-missing".to_owned(),
        "clear_jobs" => "/api/v1/jobs/clear".to_owned(),
        "browse_root" => format!(
            "/api/v1/browse?root_id={}",
            Opaque::new(SECRET).encode("r", 1)
        ),
        "browse_below" => format!(
            "/api/v1/browse/below?root_id={}",
            Opaque::new(SECRET).encode("r", 1)
        ),
        "scans" | "scan_create" => "/api/v1/scans".to_owned(),
        "jobs" => "/api/v1/jobs".to_owned(),
        "job_detail" => format!("/api/v1/jobs/{job}"),
        "job_items" => format!("/api/v1/jobs/{job}/items"),
        "item_log" => format!("/api/v1/job-items/{item}/log"),
        "file_jobs" => format!("/api/v1/files/{media}/jobs"),
        "file_detail" => format!("/api/v1/files/{media}"),
        "inspect_file" => format!("/api/v1/files/{media}/inspect"),
        "sidecars" => format!("/api/v1/files/{media}/sidecars"),
        "inspections" => format!("/api/v1/files/{media}/inspections"),
        "events" => "/api/v1/events".to_owned(),
        "job_events" => format!("/api/v1/jobs/{job}/events"),
        "schedules" | "schedule_create" => "/api/v1/schedules".to_owned(),
        "settings" | "settings_patch" => "/api/v1/settings".to_owned(),
        "settings_put" => "/api/v1/settings/max_probe_workers".to_owned(),
        "settings_audit" => "/api/v1/settings/audit".to_owned(),
        "schedule_run" => format!("/api/v1/schedules/{schedule}/run"),
        "schedule_delete" | "schedule_update" => format!("/api/v1/schedules/{schedule}"),
        "deployment" => "/api/v1/deployment".to_owned(),
        "dashboard" => "/api/v1/dashboard".to_owned(),
        "queue" => "/api/v1/dashboard/queue".to_owned(),
        "activity" => "/api/v1/dashboard/activity".to_owned(),
        _ => panic!("unknown fixture case {name}"),
    }
}

async fn assert_missing_file(app: axum::Router, opaque: &Opaque, token: &str) {
    let missing = opaque.encode("m", 999);
    let response = app
        .oneshot(
            Request::get(format!("/api/v1/files/{missing}/jobs"))
                .header("origin", "http://testserver")
                .header("cookie", format!("__Host-atvprep_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

struct FixtureTokens<'a> {
    create: &'a str,
    update: &'a str,
    run: &'a str,
    delete: &'a str,
    session: &'a str,
    settings_patch: &'a str,
    settings_put: &'a str,
    scan: &'a str,
    dismiss: &'a str,
    inspect: &'a str,
    clear: &'a str,
}

fn fixture_request(name: &str, path: String, tokens: &FixtureTokens<'_>) -> Request<Body> {
    let request = if name == "clear_jobs" {
        Request::post(path).header("x-csrf-token",tokens.clear).header("idempotency-key",name).header("content-type","application/json").body(Body::from(r#"{"statuses":["succeeded"]}"#))
    } else if name == "inspect_file" {
        Request::post(path).header("x-csrf-token",tokens.inspect).header("idempotency-key",name).body(Body::empty())
    } else if name == "dismiss_missing" {
        Request::post(path).header("x-csrf-token",tokens.dismiss).header("idempotency-key",name).body(Body::empty())
    } else if name == "scan_create" {
        Request::post(path).header("x-csrf-token",tokens.scan).header("idempotency-key",name).header("content-type","application/json").body(Body::from(json!({"root_id":Opaque::new(SECRET).encode("r",1)}).to_string()))
    } else if name == "schedule_create" {
        Request::post(path)
            .header("x-csrf-token", tokens.create)
            .header("idempotency-key", "create-schedule")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"name":"New","action":"verify","days":[0,2],"time":"12:30"}).to_string(),
            ))
    } else if name == "schedule_update" {
        Request::patch(path)
            .header("x-csrf-token", tokens.update)
            .header("idempotency-key", name)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"name":"Updated","days":[1,4],"enabled":true,"clear_root":true}).to_string(),
            ))
    } else if name == "settings_patch" {
        Request::patch(path).header("x-csrf-token",tokens.settings_patch).header("idempotency-key",name).header("content-type","application/json")
            .body(Body::from(r#"{"settings":{"scan_interval_seconds":600,"delete_original":true},"expected_revision":1}"#))
    } else if name == "settings_put" {
        Request::put(path).header("x-csrf-token",tokens.settings_put).header("idempotency-key",name).header("content-type","application/json")
            .body(Body::from(r#"{"value":4,"expected_revision":2}"#))
    } else if name == "schedule_run" {
        Request::post(path)
            .header("x-csrf-token", tokens.run)
            .header("idempotency-key", name)
            .body(Body::empty())
    } else if name == "schedule_delete" {
        Request::delete(path)
            .header("x-csrf-token", tokens.delete)
            .header("idempotency-key", name)
            .body(Body::empty())
    } else {
        Request::get(path).body(Body::empty())
    }
    .unwrap();
    let (mut parts, body) = request.into_parts();
    parts
        .headers
        .insert("origin", "http://testserver".parse().unwrap());
    parts.headers.insert(
        "cookie",
        format!("__Host-atvprep_session={}", tokens.session)
            .parse()
            .unwrap(),
    );
    Request::from_parts(parts, body)
}

fn issue_fixture_tokens(
    auth: &AuthService<'_>,
    token: &str,
    schedule: &str,
    media: &str,
) -> [String; 10] {
    [
        auth.issue_csrf(token, "PATCH", "/api/v1/settings", "settings")
            .unwrap(),
        auth.issue_csrf(
            token,
            "PUT",
            "/api/v1/settings/max_probe_workers",
            "settings",
        )
        .unwrap(),
        auth.issue_csrf(token, "POST", "/api/v1/schedules", "create-schedule")
            .unwrap(),
        auth.issue_csrf(
            token,
            "PATCH",
            &format!("/api/v1/schedules/{schedule}"),
            "update-schedule",
        )
        .unwrap(),
        auth.issue_csrf(
            token,
            "POST",
            &format!("/api/v1/schedules/{schedule}/run"),
            "run-schedule",
        )
        .unwrap(),
        auth.issue_csrf(
            token,
            "DELETE",
            &format!("/api/v1/schedules/{schedule}"),
            "delete-schedule",
        )
        .unwrap(),
        auth.issue_csrf(token, "POST", "/api/v1/scans", "scan")
            .unwrap(),
        auth.issue_csrf(
            token,
            "POST",
            "/api/v1/files/dismiss-missing",
            "dismiss-missing",
        )
        .unwrap(),
        auth.issue_csrf(
            token,
            "POST",
            &format!("/api/v1/files/{media}/inspect"),
            "inspect",
        )
        .unwrap(),
        auth.issue_csrf(token, "POST", "/api/v1/jobs/clear", "clear-jobs")
            .unwrap(),
    ]
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // The oracle cases share one initialized database and session.
async fn read_routes_match_recorded_python_responses() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    seed::apply(conn.raw());
    let config = ManagerConfig {
        api_secret: Some(SECRET.to_owned()),
        allowed_origins: vec!["http://testserver".to_owned()],
        ..ManagerConfig::default()
    };
    let state = AppState::new(db, config).unwrap();
    let auth = AuthService::with_csrf_ttl(
        Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        },
        28_800,
        3_600,
        900,
    )
    .unwrap();
    let session = auth.create().unwrap();
    let opaque = Opaque::new(SECRET);
    let schedule = opaque.encode("sc", 1);
    let media = opaque.encode("m", 1);
    let [
        settings_patch_csrf,
        settings_put_csrf,
        csrf,
        update_csrf,
        run_csrf,
        delete_csrf,
        scan_csrf,
        dismiss_csrf,
        inspect_csrf,
        clear_csrf,
    ] = issue_fixture_tokens(&auth, &session.token, &schedule, &media);
    let job = opaque.encode("j", 1);
    let item = opaque.encode("i", 1);
    let app = router(state.clone());
    let tokens = FixtureTokens {
        create: &csrf,
        update: &update_csrf,
        run: &run_csrf,
        delete: &delete_csrf,
        session: &session.token,
        settings_patch: &settings_patch_csrf,
        settings_put: &settings_put_csrf,
        scan: &scan_csrf,
        dismiss: &dismiss_csrf,
        inspect: &inspect_csrf,
        clear: &clear_csrf,
    };
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../testdata/golden/api_read_contract.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let path = fixture_path(name, &job, &item, &media, &schedule);
        let request = fixture_request(name, path, &tokens);
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let mut body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        normalise(&mut body, "", &opaque);
        assert_eq!(
            json!({"status":status,"body":body}),
            case["expected"],
            "case {name}"
        );
    }
    let replay = app
        .clone()
        .oneshot(fixture_request(
            "scan_create",
            "/api/v1/scans".to_owned(),
            &tokens,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), 201);
    let queued: i64 = conn
        .raw()
        .query_row(
            "SELECT count(*) FROM scan_runs WHERE trigger='manual' AND status='queued'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(queued, 1);
    let inspect_path = format!("/api/v1/files/{media}/inspect");
    let replay = app
        .clone()
        .oneshot(fixture_request("inspect_file", inspect_path, &tokens))
        .await
        .unwrap();
    assert_eq!(replay.status(), 201);
    let inspections: i64 = conn
        .raw()
        .query_row(
            "SELECT count(*) FROM jobs WHERE action='inspect'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(inspections, 1);
    conn.raw()
        .execute(
            "UPDATE deployment_gate SET mode='maintenance' WHERE id=1",
            [],
        )
        .unwrap();
    let fresh = auth
        .issue_csrf(&session.token, "POST", "/api/v1/scans", "scan")
        .unwrap();
    let denied = app
        .clone()
        .oneshot(
            Request::post("/api/v1/scans")
                .header("origin", "http://testserver")
                .header(
                    "cookie",
                    format!("__Host-atvprep_session={}", session.token),
                )
                .header("x-csrf-token", fresh)
                .header("idempotency-key", "new-scan")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"root_id":opaque.encode("r",1)}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), 503);
    conn.raw()
        .execute(
            "UPDATE media_files SET discovery_state='missing' WHERE id=1",
            [],
        )
        .unwrap();
    let csrf = auth
        .issue_csrf(
            &session.token,
            "POST",
            "/api/v1/files/dismiss-missing",
            "dismiss-missing",
        )
        .unwrap();
    let dismiss = || {
        Request::post("/api/v1/files/dismiss-missing")
            .header("origin", "http://testserver")
            .header(
                "cookie",
                format!("__Host-atvprep_session={}", session.token),
            )
            .header("x-csrf-token", &csrf)
            .header("idempotency-key", "dismiss-during-maintenance")
            .body(Body::empty())
            .unwrap()
    };
    let response = app.clone().oneshot(dismiss()).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["data"]["removed"],
        1
    );
    let replay = app.clone().oneshot(dismiss()).await.unwrap();
    assert_eq!(body, to_bytes(replay.into_body(), 65536).await.unwrap());
    assert_missing_file(app, &opaque, &session.token).await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One shared fixture exercises list and browse consistency.
async fn file_listing_filters_sorts_and_paginates() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private');
        INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
        (1,1,'Straße.mkv','video','.mkv',30,1,'a','present','warning','idle','none'),
        (2,1,'Strasse.mp4','video','.mp4',10,1,'b','present','clean','idle','none'),
        (3,1,'Alpha.mp4','video','.mp4',20,1,'c','present','blocker','idle','none'),
        (4,1,'Missing.mp4','video','.mp4',40,1,'d','missing','clean','idle','none');
        UPDATE media_files SET dismissed_at='2026-01-01 00:00:00' WHERE id=4;").unwrap();
    let config = ManagerConfig {
        api_secret: Some(SECRET.to_owned()),
        allowed_origins: vec!["http://testserver".to_owned()],
        ..ManagerConfig::default()
    };
    let state = AppState::new(db, config).unwrap();
    let auth = AuthService::with_csrf_ttl(
        Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        },
        28_800,
        3_600,
        900,
    )
    .unwrap();
    let session = auth.create().unwrap();
    let app = router(state);
    let request = |path: String| {
        Request::get(path)
            .header("origin", "http://testserver")
            .header(
                "cookie",
                format!("__Host-atvprep_session={}", session.token),
            )
            .body(Body::empty())
            .unwrap()
    };
    let first = app
        .clone()
        .oneshot(request("/api/v1/files?sort=title&limit=2".to_owned()))
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let first: Value =
        serde_json::from_slice(&to_bytes(first.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(first["data"][0]["display_path"], "Alpha.mp4");
    assert_eq!(first["data"][1]["display_path"], "Straße.mkv");
    assert_eq!(first["meta"]["facets"]["suffix"], json!([".mkv", ".mp4"]));
    let cursor = first["meta"]["next_cursor"].as_str().unwrap();
    let second = app
        .clone()
        .oneshot(request(format!(
            "/api/v1/files?sort=title&limit=2&cursor={cursor}"
        )))
        .await
        .unwrap();
    let second: Value =
        serde_json::from_slice(&to_bytes(second.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(second["data"][0]["display_path"], "Strasse.mp4");
    assert!(second["meta"]["next_cursor"].is_null());
    let filtered = app
        .clone()
        .oneshot(request(
            "/api/v1/files?compliance_state=converted".to_owned(),
        ))
        .await
        .unwrap();
    let filtered: Value =
        serde_json::from_slice(&to_bytes(filtered.into_body(), 65536).await.unwrap()).unwrap();
    assert!(filtered["data"].as_array().unwrap().is_empty());
    let filtered = app
        .clone()
        .oneshot(request(
            "/api/v1/files?status=warning&sort=size&dir=desc".to_owned(),
        ))
        .await
        .unwrap();
    let filtered: Value =
        serde_json::from_slice(&to_bytes(filtered.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(filtered["data"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["data"][0]["display_path"], "Straße.mkv");
    assert_eq!(
        app.clone()
            .oneshot(request("/api/v1/files?status=invalid".to_owned()))
            .await
            .unwrap()
            .status(),
        422
    );
    conn.raw()
        .execute(
            "UPDATE media_files SET relative_path='Folder/Alpha.mp4' WHERE id=3",
            [],
        )
        .unwrap();
    let root = Opaque::new(SECRET).encode("r", 1);
    let response = app
        .clone()
        .oneshot(request(format!(
            "/api/v1/browse?root_id={root}&state=blocker"
        )))
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["data"]["folders"][0]["name"], "Folder");
    assert_eq!(body["data"]["folders"][0]["counts"]["blocker"], 1);
    assert!(body["data"]["files"].as_array().unwrap().is_empty());
    let response = app
        .clone()
        .oneshot(request(format!(
            "/api/v1/browse/below?root_id={root}&path=Folder"
        )))
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["data"]["files"][0]["display_path"], "Folder/Alpha.mp4");
    assert_eq!(
        app.oneshot(request(format!(
            "/api/v1/browse/below?root_id={root}&path=../private"
        )))
        .await
        .unwrap()
        .status(),
        422
    );
}

#[tokio::test]
async fn interrupted_revision_is_flagged_until_a_later_item_or_inspection() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/private');
        INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
        (1,1,'A.mp4','video','.mp4',1,1,'a','present','warning','idle','none'),
        (2,1,'B.mp4','video','.mp4',1,1,'b','present','clean','idle','none'),
        (3,1,'C.mp4','video','.mp4',1,1,'c','present','clean','idle','none'),
        (4,1,'D.mp4','video','.mp4',1,1,'d','present','clean','idle','none'),
        (5,1,'E.mp4','video','.mp4',1,1,'e','present','clean','idle','none');
        INSERT INTO jobs(id,action,status) VALUES (1,'fix','failed'),(2,'convert','failed'),(3,'verify','succeeded'),(4,'fix','failed'),(5,'inspect','queued');
        INSERT INTO job_items(id,job_id,media_file_id,action,position,status,finished_at) VALUES
        (1,1,1,'fix',0,'interrupted','2026-01-01T10:00:00Z'),
        (2,2,2,'convert',0,'interrupted','2026-01-01T10:00:00Z'),
        (3,3,2,'verify',0,'succeeded','2026-01-01T11:00:00Z'),
        (4,4,3,'fix',0,'interrupted','2026-01-01T10:00:00Z'),
        (5,4,4,'fix',0,'succeeded','2026-01-01T10:00:00Z'),
        (6,1,5,'fix',0,'interrupted','2026-01-01T10:00:00Z'),
        (7,5,5,'inspect',0,'queued',NULL);
        INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result,created_at)
        VALUES (3,'c','c','ffprobe','1','{}','2026-01-01 12:00:00');").unwrap();
    let state = AppState::new(
        db,
        ManagerConfig {
            api_secret: Some(SECRET.to_owned()),
            allowed_origins: vec!["http://testserver".to_owned()],
            ..ManagerConfig::default()
        },
    )
    .unwrap();
    let auth = AuthService::with_csrf_ttl(
        Ctx {
            conn: conn.raw(),
            clock: state.clock.as_ref(),
        },
        28_800,
        3_600,
        900,
    )
    .unwrap();
    let session = auth.create().unwrap();
    let app = router(state);
    let get = |path: String| {
        Request::get(path)
            .header("origin", "http://testserver")
            .header(
                "cookie",
                format!("__Host-atvprep_session={}", session.token),
            )
            .body(Body::empty())
            .unwrap()
    };
    let body: Value = serde_json::from_slice(
        &to_bytes(
            app.clone()
                .oneshot(get("/api/v1/files?sort=title".to_owned()))
                .await
                .unwrap()
                .into_body(),
            65536,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let flags: Vec<(String, bool)> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["display_path"].as_str().unwrap().to_owned(),
                file["last_interrupted"].as_bool().unwrap(),
            )
        })
        .collect();
    // A: interrupted fix, nothing since. B: later verify item. C: re-inspected afterwards.
    // D: last item succeeded. E: only a queued inspection followed, which has not run yet.
    assert_eq!(
        flags,
        vec![
            ("A.mp4".to_owned(), true),
            ("B.mp4".to_owned(), false),
            ("C.mp4".to_owned(), false),
            ("D.mp4".to_owned(), false),
            ("E.mp4".to_owned(), true),
        ]
    );
    let token = Opaque::new(SECRET).encode("m", 1);
    let detail: Value = serde_json::from_slice(
        &to_bytes(
            app.oneshot(get(format!("/api/v1/files/{token}")))
                .await
                .unwrap()
                .into_body(),
            65536,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(detail["data"]["last_interrupted"], true);
}

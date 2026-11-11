//! "Embed open" in the list, counters, filters and the schedule hit count: files with an embeddable
//! sidecar are not "ready"; the stored state stays `clean`.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_manager::{db::Db, repo::Ctx, schema};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::Value;
use tower::ServiceExt;

const SECRET: &str = "test-api-secret";

struct Api {
    app: axum::Router,
    session: String,
}

impl Api {
    async fn get(&self, path: &str) -> Value {
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
        assert_eq!(response.status(), 200, "{path}");
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
    }

    /// Display paths of the files the list returns for a filter.
    async fn names(&self, query: &str) -> Vec<String> {
        let body = self.get(&format!("/api/v1/files?{query}")).await;
        let mut names: Vec<String> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["display_path"].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    }
}

/// 1 ready without sidecar, 2 clean with open sidecar, 3 warning with open sidecar,
/// 4 clean with an already embedded sidecar; plus a repair schedule for the library.
fn api() -> Api {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw()
        .execute_batch(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/library');
             INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
               (1,1,'ready.mp4','video','.mp4',1,1,'f1','present','clean','idle','none'),
               (2,1,'open.mp4','video','.mp4',1,1,'f2','present','clean','idle','none'),
               (3,1,'warn.mp4','video','.mp4',1,1,'f3','present','warning','idle','none'),
               (4,1,'done.mp4','video','.mp4',1,1,'f4','present','clean','idle','none');
             INSERT INTO sidecars(media_file_id,relative_path,fingerprint) VALUES
               (2,'open.de.srt','x'),(3,'warn.de.srt','x'),(4,'done.de.srt','x');
             INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled,armed_at)
               VALUES(1,'Fix','prepare',1,127,'12:00',5,0,'2000-01-01');
             INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result) VALUES
               (2,'s','f2','ffprobe','1','{\"streams\":[]}'),
               (3,'s','f3','ffprobe','1','{\"streams\":[]}'),
               (4,'s','f4','ffprobe','1','{\"streams\":[{\"codec_type\":\"subtitle\",\"codec_name\":\"mov_text\",\"tags\":{\"language\":\"deu\"}}]}');
             UPDATE media_files SET encode_need='video' WHERE id IN (3,4);
             UPDATE media_files SET encode_need='none' WHERE id IN (1,2);",
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
    .unwrap()
    .token;
    Api {
        app: router(state),
        session,
    }
}

#[tokio::test]
async fn ready_no_longer_counts_files_with_an_open_embed() {
    let api = api();
    let summary = api.get("/api/v1/dashboard/summary").await;
    let files = &summary["data"]["files"];
    assert_eq!(files["clean"], 2, "ready.mp4 and done.mp4");
    assert_eq!(files["embed_open"], 1, "open.mp4");
    assert_eq!(files["warning"], 1);
    assert_eq!(
        summary["data"]["embed"], 2,
        "open.mp4 and warn.mp4 have an open embed"
    );
}

#[tokio::test]
async fn the_ready_filter_leaves_out_open_embeds_and_a_filter_finds_them() {
    let api = api();
    assert_eq!(
        api.names("compliance_state=clean").await,
        ["done.mp4", "ready.mp4"]
    );
    assert_eq!(
        api.names("compliance_state=embed").await,
        ["open.mp4", "warn.mp4"]
    );
    // Hiding "ready" hides only the truly ready files.
    assert_eq!(api.names("hide=clean").await, ["open.mp4", "warn.mp4"]);
}

impl Api {
    async fn status(&self, path: &str) -> u16 {
        self.app
            .clone()
            .oneshot(
                Request::get(path)
                    .header("origin", "http://testserver")
                    .header("cookie", format!("__Host-atvprep_session={}", self.session))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
            .as_u16()
    }
}

#[tokio::test]
async fn properties_show_only_files_that_have_all_of_them() {
    let api = api();
    // No selection: nothing is filtered.
    assert_eq!(
        api.names("has=").await,
        ["done.mp4", "open.mp4", "ready.mp4", "warn.mp4"]
    );
    // One trait shows only files that have it.
    assert_eq!(api.names("has=embed").await, ["open.mp4", "warn.mp4"]);
    assert_eq!(
        api.names("has=encode_video").await,
        ["done.mp4", "warn.mp4"]
    );
    // Two traits show the intersection (AND).
    assert_eq!(api.names("has=embed,encode_video").await, ["warn.mp4"]);
    // Combined with hiding states.
    assert_eq!(api.names("has=embed&hide=warning").await, ["open.mp4"]);
    assert_eq!(
        api.names("has=embed,encode_video&hide=warning").await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn hide_takes_states_only_and_has_takes_properties_only() {
    let api = api();
    assert_eq!(api.status("/api/v1/files?hide=embed").await, 422);
    assert_eq!(api.status("/api/v1/files?hide=encode_video").await, 422);
    assert_eq!(api.status("/api/v1/files?has=clean").await, 422);
    assert_eq!(api.status("/api/v1/files?has=nonsense").await, 422);
    assert_eq!(api.status("/api/v1/browse?hide=embed").await, 422);
    assert_eq!(api.status("/api/v1/browse?has=clean").await, 422);
    assert_eq!(api.status("/api/v1/browse?has=embed").await, 200);
}

#[tokio::test]
async fn property_counts_follow_the_current_selection() {
    let api = api();
    let counts = |body: Value| {
        let data = &body["data"];
        ["convert", "encode_video", "embed", "converted"].map(|key| data[key].as_u64().unwrap())
    };
    // No selection: each trait on its own.
    assert_eq!(counts(api.get("/api/v1/files/counts").await), [0, 2, 2, 0]);
    // With "embed open" selected, each trait yields the intersection.
    assert_eq!(
        counts(api.get("/api/v1/files/counts?has=embed").await),
        [0, 1, 2, 0]
    );
    // Hiding states applies as well.
    assert_eq!(
        counts(api.get("/api/v1/files/counts?hide=warning").await),
        [0, 1, 1, 0]
    );
}

#[tokio::test]
async fn folder_counts_do_not_call_an_open_embed_ready() {
    let api = api();
    let root = Opaque::new(SECRET.as_bytes()).encode("r", 1);
    let body = api.get(&format!("/api/v1/browse?root_id={root}")).await;
    let counts = &body["data"]["files"];
    assert!(
        counts
            .as_array()
            .unwrap()
            .iter()
            .all(|file| file["compliance_state"] != "embed")
    );
    let root_counts = api.get("/api/v1/browse").await;
    let tally = &root_counts["data"]["folders"][0]["counts"];
    assert_eq!(tally["clean"], 2);
    assert_eq!(tally["embed_open"], 1);
    assert_eq!(tally["warning"], 1);
}

#[tokio::test]
async fn a_file_reports_its_open_embed_in_the_detail() {
    let api = api();
    let opaque = Opaque::new(SECRET.as_bytes());
    let open = api
        .get(&format!("/api/v1/files/{}", opaque.encode("m", 2)))
        .await;
    assert_eq!(open["data"]["compliance_state"], "clean");
    assert_eq!(open["data"]["pending_sidecars"], 1);
    let done = api
        .get(&format!("/api/v1/files/{}", opaque.encode("m", 4)))
        .await;
    assert_eq!(done["data"]["pending_sidecars"], 0);
}

#[tokio::test]
async fn a_repair_schedule_counts_clean_files_with_an_open_embed() {
    let api = api();
    let body = api.get("/api/v1/schedules").await;
    // open.mp4 (clean, embed open); warn.mp4 needs a re-encode and does not count, nor do ready.mp4 and done.mp4.
    assert_eq!(body["data"][0]["matching"], 1);
}

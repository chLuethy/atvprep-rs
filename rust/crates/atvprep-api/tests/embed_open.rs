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
    api_with("")
}

/// The fixture of `api` plus more SQL run after it.
fn api_with(extra: &str) -> Api {
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
    conn.raw().execute_batch(extra).unwrap();
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
    // Showing the other states leaves out only the truly ready files.
    assert_eq!(
        api.names("status=embed_open,warning").await,
        ["open.mp4", "warn.mp4"]
    );
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
    // Combined with chosen states.
    assert_eq!(api.names("has=embed&status=embed_open").await, ["open.mp4"]);
    assert_eq!(
        api.names("has=embed,encode_video&status=clean,embed_open")
            .await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn status_shows_only_the_chosen_states_and_nothing_chosen_shows_all() {
    let api = api();
    assert_eq!(
        api.names("status=").await,
        ["done.mp4", "open.mp4", "ready.mp4", "warn.mp4"]
    );
    assert_eq!(api.names("status=clean").await, ["done.mp4", "ready.mp4"]);
    // Several states are OR, in any order and with repeats.
    assert_eq!(
        api.names("status=warning,clean,warning").await,
        ["done.mp4", "ready.mp4", "warn.mp4"]
    );
    assert_eq!(api.names("status=blocker").await, Vec::<String>::new());
}

#[tokio::test]
async fn status_takes_states_only_and_has_takes_properties_only() {
    let api = api();
    assert_eq!(api.status("/api/v1/files?status=embed").await, 422);
    assert_eq!(api.status("/api/v1/files?status=encode_video").await, 422);
    assert_eq!(api.status("/api/v1/files?has=clean").await, 422);
    assert_eq!(api.status("/api/v1/files?has=nonsense").await, 422);
    assert_eq!(api.status("/api/v1/browse?status=embed").await, 422);
    assert_eq!(api.status("/api/v1/browse?has=clean").await, 422);
    assert_eq!(api.status("/api/v1/browse?has=embed").await, 200);
    assert_eq!(api.status("/api/v1/files/counts?status=embed").await, 422);
}

#[tokio::test]
async fn hide_is_no_longer_accepted() {
    let api = api();
    assert_eq!(api.status("/api/v1/files?hide=clean").await, 422);
    assert_eq!(api.status("/api/v1/browse?hide=clean").await, 422);
    assert_eq!(api.status("/api/v1/files/counts?hide=clean").await, 422);
}

/// The facet counters: per state with every filter but the states, per property with every filter
/// plus that property, and how many files match all filters out of how many there are.
fn facets(body: &Value) -> (Vec<u64>, Vec<u64>, u64, u64) {
    let data = &body["data"];
    let states = STATES.map(|key| data["status"][key].as_u64().unwrap());
    let traits = [
        "convert",
        "encode_audio",
        "encode_video",
        "embed",
        "converted",
    ]
    .map(|key| data["has"][key].as_u64().unwrap());
    (
        states.to_vec(),
        traits.to_vec(),
        data["matched"].as_u64().unwrap(),
        data["total"].as_u64().unwrap(),
    )
}

#[tokio::test]
async fn facet_counts_follow_the_other_filters() {
    let api = api();
    // Order of STATES: clean, embed_open, warning, blocker, unknown, probe_failed, missing.
    assert_eq!(
        facets(&api.get("/api/v1/files/counts").await),
        (vec![2, 1, 1, 0, 0, 0, 0], vec![0, 0, 2, 2, 0], 4, 4)
    );
    // A property narrows the state counters; each property yields the intersection.
    assert_eq!(
        facets(&api.get("/api/v1/files/counts?has=encode_video").await),
        (vec![1, 0, 1, 0, 0, 0, 0], vec![0, 0, 2, 1, 0], 2, 4)
    );
    // The chosen states do not narrow their own counters, only the properties and the match.
    assert_eq!(
        facets(&api.get("/api/v1/files/counts?status=warning").await),
        (vec![2, 1, 1, 0, 0, 0, 0], vec![0, 0, 1, 1, 0], 1, 4)
    );
    // The search narrows every counter but not the total.
    assert_eq!(
        facets(&api.get("/api/v1/files/counts?q=open").await),
        (vec![0, 1, 0, 0, 0, 0, 0], vec![0, 0, 0, 1, 0], 1, 4)
    );
}

#[tokio::test]
async fn the_column_view_takes_the_same_status_list() {
    let api = api();
    let root = Opaque::new(SECRET.as_bytes()).encode("r", 1);
    let body = api
        .get(&format!(
            "/api/v1/browse/below?root_id={root}&status=clean,warning&has=encode_video"
        ))
        .await;
    let mut names: Vec<&str> = body["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["display_path"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["done.mp4", "warn.mp4"]);
    let tally = &api.get("/api/v1/browse?status=embed_open").await["data"]["folders"][0]["counts"];
    assert_eq!(tally, &serde_json::json!({"embed_open": 1}));
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

#[tokio::test]
async fn embed_open_is_a_state_of_its_own_that_filters() {
    let api = api();
    assert_eq!(api.names("compliance_state=embed_open").await, ["open.mp4"]);
    assert_eq!(
        api.names("status=clean,warning").await,
        ["done.mp4", "ready.mp4", "warn.mp4"]
    );
    let root = Opaque::new(SECRET.as_bytes()).encode("r", 1);
    let body = api
        .get(&format!(
            "/api/v1/browse/below?root_id={root}&state=embed_open"
        ))
        .await;
    let names: Vec<&str> = body["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["display_path"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["open.mp4"]);
}

const STATES: [&str; 7] = [
    "clean",
    "embed_open",
    "warning",
    "blocker",
    "unknown",
    "probe_failed",
    "missing",
];

/// The fixture of `api` plus a missing file whose last verdict was a warning.
fn api_with_missing() -> Api {
    api_with(
        "INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action)
           VALUES (5,1,'gone.mp4','video','.mp4',1,1,'f5','missing','warning','idle','none');",
    )
}

#[tokio::test]
async fn the_states_split_the_files_without_gaps_or_overlap() {
    let api = api_with_missing();
    let mut seen = Vec::new();
    for state in STATES {
        seen.extend(api.names(&format!("compliance_state={state}")).await);
    }
    seen.sort();
    assert_eq!(
        seen,
        ["done.mp4", "gone.mp4", "open.mp4", "ready.mp4", "warn.mp4"]
    );
    assert_eq!(api.names("compliance_state=warning").await, ["warn.mp4"]);
    assert_eq!(api.names("compliance_state=missing").await, ["gone.mp4"]);
}

#[tokio::test]
async fn the_state_counters_add_up_to_all_files() {
    let api = api_with_missing();
    let summary = api.get("/api/v1/dashboard/summary").await;
    let files = summary["data"]["files"].as_object().unwrap();
    assert_eq!(files["clean"], 2);
    assert_eq!(files["embed_open"], 1);
    assert_eq!(files["warning"], 1, "gone.mp4 counts as missing only");
    assert_eq!(files["missing"], 1);
    assert!(files.keys().all(|key| STATES.contains(&key.as_str())));
    let total: u64 = files.values().map(|n| n.as_u64().unwrap()).sum();
    assert_eq!(total, 5);
}

impl Api {
    /// Display paths in the order the list returns them.
    async fn ordered(&self, query: &str) -> Vec<String> {
        let body = self.get(&format!("/api/v1/files?{query}")).await;
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["display_path"].as_str().unwrap().to_owned())
            .collect()
    }
}

#[tokio::test]
async fn the_status_sort_places_missing_and_embed_open() {
    // Order: blocker, inspection failed, missing, warning, embed open, not verified, ready.
    let api = api_with(
        "INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
           (5,1,'gone.mp4','video','.mp4',1,1,'f5','missing','clean','idle','none'),
           (6,1,'blocked.mp4','video','.mp4',1,1,'f6','present','blocker','idle','none'),
           (7,1,'new.mp4','video','.mp4',1,1,'f7','present','unknown','idle','none'),
           (8,1,'broken.mp4','video','.mp4',1,1,'f8','present','probe_failed','idle','none');",
    );
    let order = [
        "blocked.mp4",
        "broken.mp4",
        "gone.mp4",
        "warn.mp4",
        "open.mp4",
        "new.mp4",
        "done.mp4",
        "ready.mp4",
    ];
    assert_eq!(api.ordered("sort=status&dir=asc").await, order);
    // Descending reverses the states; names stay A to Z within one.
    assert_eq!(
        api.ordered("sort=status&dir=desc").await,
        [
            "done.mp4",
            "ready.mp4",
            "new.mp4",
            "open.mp4",
            "warn.mp4",
            "gone.mp4",
            "broken.mp4",
            "blocked.mp4",
        ]
    );
    // Paging keeps the order.
    let first = api.get("/api/v1/files?sort=status&limit=3").await;
    let cursor = first["meta"]["next_cursor"].as_str().unwrap().to_owned();
    assert_eq!(
        api.ordered(&format!("sort=status&limit=3&cursor={cursor}"))
            .await,
        order[3..6]
    );
}

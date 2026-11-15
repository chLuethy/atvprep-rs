//! The file state at the read API: every file row shows one `state` token, and filter, counters,
//! overview, sorting and column view agree on it.
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
}

/// 1 ready without sidecar, 2 clean with open sidecar, 3 warning with open sidecar, 4 clean with an
/// already embedded sidecar, plus the files of `extra` SQL run after it.
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

const MORE_FILES: &str = "INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
           (5,1,'gone.mp4','video','.mp4',1,1,'f5','missing','clean','idle','none'),
           (6,1,'blocked.mp4','video','.mp4',1,1,'f6','present','blocker','idle','none'),
           (7,1,'new.mp4','video','.mp4',1,1,'f7','present','unknown','idle','fix'),
           (8,1,'broken.mp4','video','.mp4',1,1,'f8','present','probe_failed','idle','none');";

type Mark = (String, String, bool);

/// `display_path`, `state` and `preparable` of the file rows, sorted by name.
fn marks(files: &Value) -> Vec<Mark> {
    let mut marks: Vec<Mark> = files
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["display_path"].as_str().unwrap().to_owned(),
                file["state"].as_str().unwrap().to_owned(),
                file["preparable"].as_bool().unwrap(),
            )
        })
        .collect();
    marks.sort();
    marks
}

fn mark(name: &str, state: &str, preparable: bool) -> Mark {
    (name.to_owned(), state.to_owned(), preparable)
}

fn paths(files: &Value) -> Vec<String> {
    files
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["display_path"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn every_file_row_carries_its_state_and_whether_prepare_takes_it() {
    let api = api_with(MORE_FILES);
    let expected = vec![
        mark("blocked.mp4", "blocker", true),
        mark("broken.mp4", "probe_failed", false),
        mark("done.mp4", "clean", false),
        mark("gone.mp4", "missing", false),
        mark("new.mp4", "unknown", true),
        mark("open.mp4", "embed_open", true),
        mark("ready.mp4", "clean", false),
        mark("warn.mp4", "warning", true),
    ];
    assert_eq!(marks(&api.get("/api/v1/files").await["data"]), expected);
    let root = Opaque::new(SECRET.as_bytes()).encode("r", 1);
    let below = api
        .get(&format!("/api/v1/browse/below?root_id={root}"))
        .await;
    assert_eq!(marks(&below["data"]["files"]), expected);
    let detail = api
        .get(&format!(
            "/api/v1/files/{}",
            Opaque::new(SECRET.as_bytes()).encode("m", 2)
        ))
        .await;
    assert_eq!(detail["data"]["state"], "embed_open");
    assert_eq!(detail["data"]["preparable"], true);
}

#[tokio::test]
async fn the_facet_counters_the_overview_and_the_total_agree() {
    let api = api_with(MORE_FILES);
    let counts = api.get("/api/v1/files/counts").await;
    let overview = api.get("/api/v1/dashboard/summary").await;
    let mut sum = 0;
    for (token, n) in counts["data"]["status"].as_object().unwrap() {
        let n = n.as_i64().unwrap();
        sum += n;
        let tile = overview["data"]["files"]
            .get(token)
            .and_then(Value::as_i64)
            .unwrap_or(0);
        assert_eq!(tile, n, "{token}");
    }
    assert_eq!(sum, counts["data"]["total"].as_i64().unwrap());
    assert_eq!(sum, 8);
}

#[tokio::test]
async fn the_status_sort_is_the_same_in_the_list_and_the_column_view() {
    let api = api_with(MORE_FILES);
    let root = Opaque::new(SECRET.as_bytes()).encode("r", 1);
    for dir in ["asc", "desc"] {
        let list = api
            .get(&format!("/api/v1/files?sort=status&dir={dir}"))
            .await;
        let column = api
            .get(&format!(
                "/api/v1/browse?root_id={root}&sort=status&dir={dir}"
            ))
            .await;
        assert_eq!(
            paths(&list["data"]),
            paths(&column["data"]["files"]),
            "{dir}"
        );
    }
}

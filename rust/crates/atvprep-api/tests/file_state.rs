//! The file state at the read API: every file row shows one `state` token, and filter, counters,
//! overview, sorting and column view agree on it.
use atvprep_api::{
    auth::AuthService, config::ManagerConfig, opaque::Opaque, router, state::AppState,
};
use atvprep_fixtures::{Fixtures, Verdict};
use atvprep_manager::{db::Db, repo::Ctx, schema};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
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
/// already embedded sidecar, and four files in the other states.
fn api() -> Api {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    {
        let fx = Fixtures::new(conn.raw());
        let root = fx.library_at("Movies", "/library");
        let file = |name: &str, fingerprint: &str, verdict: Verdict, need: &'static str| {
            fx.file(root, name)
                .fingerprint(fingerprint)
                .verdict(verdict)
                .encode(need, "none")
                .create()
        };
        let _ = file("ready.mp4", "f1", Verdict::Clean, "none");
        let open = file("open.mp4", "f2", Verdict::Clean, "none");
        let warn = file("warn.mp4", "f3", Verdict::Warning, "video");
        let done = file("done.mp4", "f4", Verdict::Clean, "video");
        for (media, sidecar) in [
            (open, "open.de.srt"),
            (warn, "warn.de.srt"),
            (done, "done.de.srt"),
        ] {
            let _ = fx.sidecar(media, sidecar, "x");
        }
        // The streams a file was probed with: only `done.mp4` already carries the subtitle.
        let streams = |media: i64, fingerprint: &str, result: Value| {
            let _ = fx.inspection(media, "ffprobe", fingerprint, &result);
        };
        streams(open, "f2", json!({"streams": []}));
        streams(warn, "f3", json!({"streams": []}));
        streams(
            done,
            "f4",
            json!({"streams": [{"codec_type": "subtitle", "codec_name": "mov_text", "tags": {"language": "deu"}}]}),
        );
        let _ = fx
            .schedule("Fix", "prepare")
            .library(root)
            .days_mask(127)
            .time_of_day("12:00")
            .max_files(5)
            .enabled(false)
            .armed_at("2000-01-01")
            .create();
        let _ = fx
            .file(root, "gone.mp4")
            .fingerprint("f5")
            .verdict(Verdict::Clean)
            .missing()
            .create();
        let _ = fx
            .file(root, "blocked.mp4")
            .fingerprint("f6")
            .verdict(Verdict::Blocker)
            .create();
        let _ = fx
            .file(root, "new.mp4")
            .fingerprint("f7")
            .desired_action("fix")
            .create();
        let _ = fx
            .file(root, "broken.mp4")
            .fingerprint("f8")
            .inspection_failed()
            .create();
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
    .unwrap()
    .token;
    Api {
        app: router(state),
        session,
    }
}

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
    let api = api();
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
    let api = api();
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
    let api = api();
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

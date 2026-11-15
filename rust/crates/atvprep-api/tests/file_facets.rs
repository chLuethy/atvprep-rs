//! The facets of the file list besides states and properties: library (`root_id`), extension
//! (`suffix`) and subtitle files (`has_sidecar`), their counters, the order "recently added" and the
//! same filters in the column view.
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
    async fn request(&self, path: &str) -> (u16, Value) {
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
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> Value {
        let (status, body) = self.request(path).await;
        assert_eq!(status, 200, "{path}: {body}");
        body
    }

    /// Display paths in the order the list returns them.
    async fn ordered(&self, query: &str) -> Vec<String> {
        self.get(&format!("/api/v1/files?{query}")).await["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["display_path"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Display paths the list returns, sorted.
    async fn names(&self, query: &str) -> Vec<String> {
        let mut names = self.ordered(query).await;
        names.sort();
        names
    }

    async fn counts(&self, query: &str) -> Value {
        self.get(&format!("/api/v1/files/counts?{query}")).await["data"].clone()
    }
}

fn root(id: i64) -> String {
    Opaque::new(SECRET.as_bytes()).encode("r", id)
}

/// Movies (1): a.mp4 (ready), b.mkv (warning, subtitle file), Sub/c.mkv (blocker).
/// Shows (2): d.avi (warning, subtitle file), e.mp4 (not verified).
/// A third library, Empty (3), has no files.
fn api() -> Api {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let fx = Fixtures::new(conn.raw());
    let movies = fx.library("Movies");
    let shows = fx.library("Shows");
    let _ = fx.library("Empty");
    let _ = fx
        .file(movies, "a.mp4")
        .fingerprint("f1")
        .verdict(Verdict::Clean)
        .create();
    let b = fx
        .file(movies, "b.mkv")
        .fingerprint("f2")
        .verdict(Verdict::Warning)
        .create();
    let _ = fx
        .file(movies, "Sub/c.mkv")
        .fingerprint("f3")
        .verdict(Verdict::Blocker)
        .create();
    let d = fx
        .file(shows, "d.avi")
        .fingerprint("f4")
        .verdict(Verdict::Warning)
        .create();
    let _ = fx.file(shows, "e.mp4").fingerprint("f5").create();
    let _ = fx.sidecar(b, "b.de.srt", "x");
    let _ = fx.sidecar(d, "d.en.srt", "y");
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
async fn library_extension_and_subtitle_files_filter_the_list() {
    let api = api();
    let (movies, shows) = (root(1), root(2));
    assert_eq!(
        api.names(&format!("root_id={shows}")).await,
        ["d.avi", "e.mp4"]
    );
    // Several libraries or extensions show any of them.
    assert_eq!(
        api.names(&format!("root_id={movies},{shows}")).await.len(),
        5
    );
    assert_eq!(
        api.names("suffix=.mkv,.avi").await,
        ["Sub/c.mkv", "b.mkv", "d.avi"]
    );
    assert_eq!(api.names("has_sidecar=true").await, ["b.mkv", "d.avi"]);
    assert_eq!(
        api.names("has_sidecar=false").await,
        ["Sub/c.mkv", "a.mp4", "e.mp4"]
    );
    assert_eq!(
        api.names(&format!("root_id={movies}&suffix=.mkv&has_sidecar=false"))
            .await,
        ["Sub/c.mkv"]
    );
}

#[tokio::test]
async fn the_counters_cover_library_extension_and_subtitle_files() {
    let api = api();
    let (movies, shows, empty) = (root(1), root(2), root(3));
    let all = api.counts("").await;
    assert_eq!(all["root_id"], json!({ &movies: 3, &shows: 2, &empty: 0 }));
    assert_eq!(all["suffix"], json!({".avi": 1, ".mkv": 2, ".mp4": 2}));
    assert_eq!(all["has_sidecar"], json!({"true": 2, "false": 3}));

    // Each facet counts with every other filter, but not with its own choice.
    let narrowed = api.counts(&format!("root_id={movies}&suffix=.mkv")).await;
    assert_eq!(
        narrowed["root_id"],
        json!({ &movies: 2, &shows: 0, &empty: 0 })
    );
    // An extension that no longer has a hit stays listed with 0.
    assert_eq!(narrowed["suffix"], json!({".avi": 0, ".mkv": 2, ".mp4": 1}));
    assert_eq!(narrowed["has_sidecar"], json!({"true": 1, "false": 1}));
    assert_eq!(narrowed["matched"], 2);
    assert_eq!(narrowed["total"], 5);
    // The other facets narrow the states and properties too.
    assert_eq!(narrowed["status"]["blocker"], 1);
    assert_eq!(narrowed["status"]["clean"], 0);

    let sidecar = api.counts("has_sidecar=true").await;
    assert_eq!(sidecar["has_sidecar"], json!({"true": 2, "false": 3}));
    assert_eq!(sidecar["suffix"], json!({".avi": 1, ".mkv": 1, ".mp4": 0}));
    assert_eq!(sidecar["status"]["warning"], 2);
}

#[tokio::test]
async fn bad_facet_values_are_refused() {
    let api = api();
    assert_eq!(api.request("/api/v1/files?has_sidecar=maybe").await.0, 422);
    assert_eq!(
        api.request("/api/v1/files/counts?has_sidecar=maybe")
            .await
            .0,
        422
    );
    assert_eq!(api.request("/api/v1/files?root_id=nonsense").await.0, 404);
    assert_eq!(
        api.request("/api/v1/files/counts?root_id=nonsense").await.0,
        404
    );
    let long = ".mkv,".repeat(30);
    assert_eq!(
        api.request(&format!("/api/v1/files?suffix={long}")).await.0,
        422
    );
}

#[tokio::test]
async fn recently_added_is_the_order_without_sort_in_both_directions() {
    let api = api();
    assert_eq!(
        api.ordered("").await,
        ["a.mp4", "b.mkv", "Sub/c.mkv", "d.avi", "e.mp4"]
    );
    assert_eq!(
        api.ordered("dir=desc").await,
        ["e.mp4", "d.avi", "Sub/c.mkv", "b.mkv", "a.mp4"]
    );
    // Newest first also pages: the cursor goes on from where the page stopped.
    let first = api.get("/api/v1/files?dir=desc&limit=2").await;
    let names: Vec<&str> = first["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["display_path"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["e.mp4", "d.avi"]);
    let cursor = first["meta"]["next_cursor"].as_str().unwrap();
    assert_eq!(
        api.ordered(&format!("dir=desc&limit=2&cursor={cursor}"))
            .await,
        ["Sub/c.mkv", "b.mkv"]
    );
}

#[tokio::test]
async fn the_column_view_takes_the_same_facets_and_hides_empty_folders() {
    let api = api();
    let movies = root(1);
    // Libraries without a hit are left out, the others count only hits.
    let top = api.get("/api/v1/browse?suffix=.mkv").await;
    assert_eq!(top["data"]["folders"].as_array().unwrap().len(), 1);
    assert_eq!(top["data"]["folders"][0]["name"], "Movies");
    assert_eq!(top["data"]["folders"][0]["total"], 2);
    // Inside a library a folder without a hit is left out as well.
    let level = api
        .get(&format!("/api/v1/browse?root_id={movies}&has_sidecar=true"))
        .await;
    assert_eq!(level["data"]["folders"], json!([]));
    let files: Vec<&str> = level["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["display_path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["b.mkv"]);
    let below = api
        .get(&format!(
            "/api/v1/browse/below?root_id={movies}&suffix=.mp4"
        ))
        .await;
    assert_eq!(below["data"]["files"].as_array().unwrap().len(), 1);
    assert_eq!(api.request("/api/v1/browse?has_sidecar=maybe").await.0, 422);
}

#[tokio::test]
async fn invalid_input_is_refused_before_an_unknown_library_is_missing_on_the_file_list() {
    // As before the facet engine: every invalid value (a facet token, the date) answers 422 first.
    let api = api();
    for path in [
        "/api/v1/files?root_id=nonsense&status=nonsense",
        "/api/v1/files?root_id=nonsense&last_seen_from=yesterday",
        "/api/v1/files?root_id=nonsense&last_seen_to=yesterday",
        "/api/v1/files?last_seen_from=yesterday",
    ] {
        assert_eq!(api.request(path).await.0, 422, "{path}");
    }
}

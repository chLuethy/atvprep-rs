//! The period (`from`/`to`, on when a job was queued) and the library (`root_id`) of the job list,
//! and the library counter of `/jobs/counts`.
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

    async fn counts(&self, query: &str) -> Value {
        self.get(&format!("/api/v1/jobs/counts?{query}")).await["data"].clone()
    }
}

fn root(id: i64) -> String {
    Opaque::new(SECRET.as_bytes()).encode("r", id)
}

/// Movies (1) and Shows (2), a third library Empty (3) without files. Jobs, by when they were
/// queued (stored in both formats the database holds):
/// 1 `2026-11-12 08:00:00` Movies/a, 2 `2026-11-13T23:30:00+00:00` Shows/d,
/// 3 `2026-11-14 00:15:00.000000` Movies/b then Shows/e (both libraries), 4 `2026-11-14T09:00:00Z`
/// Shows/e.
fn api() -> Api {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let fx = Fixtures::new(conn.raw());
    let movies = fx.library("Movies");
    let shows = fx.library("Shows");
    let _ = fx.library("Empty");
    let file = |root, path: &str, fingerprint: &str| {
        fx.file(root, path)
            .fingerprint(fingerprint)
            .verdict(Verdict::Clean)
            .create()
    };
    let a = file(movies, "a.mkv", "f1");
    let b = file(movies, "b.mkv", "f2");
    let d = file(shows, "d.mkv", "f4");
    let e = file(shows, "e.mkv", "f5");
    let _ = fx
        .job("verify", &[a])
        .status("succeeded")
        .requested_at("2026-11-12 08:00:00")
        .create();
    let _ = fx
        .job("verify", &[d])
        .status("failed")
        .requested_at("2026-11-13T23:30:00+00:00")
        .create();
    let _ = fx
        .job("verify", &[b, e])
        .status("succeeded")
        .requested_at("2026-11-14 00:15:00.000000")
        .create();
    let _ = fx
        .job("verify", &[e])
        .requested_at("2026-11-14T09:00:00Z")
        .create();
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
async fn from_is_inclusive_and_to_exclusive_on_when_a_job_was_queued() {
    let api = api();
    assert_eq!(
        api.subjects("from=2026-11-13T23:30:00Z").await,
        ["d.mkv", "b.mkv", "e.mkv"]
    );
    assert_eq!(
        api.subjects("from=2026-11-13T00:00:00Z&to=2026-11-14T00:15:00Z")
            .await,
        ["d.mkv"]
    );
    assert_eq!(api.subjects("to=2026-11-13T00:00:00Z").await, ["a.mkv"]);
}

#[tokio::test]
async fn the_period_takes_any_offset_and_compares_instants() {
    let api = api();
    // Midnight in Zurich (UTC+1) is 23:00 UTC the day before.
    assert_eq!(
        api.subjects("from=2026-11-14T00:00:00%2B01:00").await,
        ["d.mkv", "b.mkv", "e.mkv"]
    );
    assert_eq!(
        api.subjects("from=2026-11-14T00:00:00.000Z").await,
        ["b.mkv", "e.mkv"]
    );
}

#[tokio::test]
async fn a_job_belongs_to_every_library_one_of_its_files_lies_in() {
    let api = api();
    let (movies, shows) = (root(1), root(2));
    assert_eq!(
        api.subjects(&format!("root_id={movies}")).await,
        ["a.mkv", "b.mkv"]
    );
    assert_eq!(
        api.subjects(&format!("root_id={shows}")).await,
        ["d.mkv", "b.mkv", "e.mkv"]
    );
    assert_eq!(
        api.subjects(&format!("root_id={movies},{shows}"))
            .await
            .len(),
        4
    );
    assert_eq!(
        api.subjects(&format!(
            "root_id={shows}&from=2026-11-14T00:00:00Z&status=queued"
        ))
        .await,
        ["e.mkv"]
    );
}

#[tokio::test]
async fn counts_tell_per_library_what_it_would_show_with_every_other_filter() {
    let api = api();
    let (movies, shows, empty) = (root(1), root(2), root(3));
    let all = api.counts("").await;
    assert_eq!(all["root_id"], json!({ &movies: 2, &shows: 3, &empty: 0 }));
    assert_eq!(all["matched"], 4);

    let narrowed = api
        .counts(&format!("root_id={movies}&from=2026-11-13T00:00:00Z"))
        .await;
    // The library counter ignores the chosen library, not the period.
    assert_eq!(
        narrowed["root_id"],
        json!({ &movies: 1, &shows: 3, &empty: 0 })
    );
    assert_eq!(narrowed["matched"], 1);
    assert_eq!(narrowed["total"], 4);
    // Every other counter applies the period and the library.
    assert_eq!(narrowed["status"]["succeeded"], 1);
    assert_eq!(narrowed["status"]["queued"], 0);
}

#[tokio::test]
async fn a_bad_period_or_library_is_refused() {
    let api = api();
    for query in [
        "from=yesterday",
        "to=2026-11-14",
        "from=2026-11-14T25:00:00Z",
    ] {
        for path in ["/api/v1/jobs", "/api/v1/jobs/counts"] {
            let (status, body) = api.call(&format!("{path}?{query}")).await;
            assert_eq!(status, 422, "{path}?{query}: {body}");
        }
    }
}

#[tokio::test]
async fn an_unknown_library_is_not_found_as_on_the_file_list() {
    let api = api();
    for path in ["/api/v1/jobs", "/api/v1/jobs/counts"] {
        let (status, body) = api.call(&format!("{path}?root_id=nonsense")).await;
        assert_eq!(status, 404, "{path}: {body}");
    }
}

#[tokio::test]
async fn an_unknown_library_is_found_missing_before_any_other_bad_value_of_the_job_list() {
    // The order of the refusals is the one from before the facet engine: the library is resolved first.
    let api = api();
    for path in ["/api/v1/jobs", "/api/v1/jobs/counts"] {
        for other in [
            "status=nonsense",
            "action=nonsense",
            "from=yesterday",
            "trigger=nonsense",
        ] {
            let (status, body) = api.call(&format!("{path}?root_id=nonsense&{other}")).await;
            assert_eq!(status, 404, "{path}?{other}: {body}");
        }
        // A library list that is too long is still invalid input, not missing.
        let long = "r,".repeat(2000);
        let (status, _) = api.call(&format!("{path}?root_id={long}")).await;
        assert_eq!(status, 422, "{path}");
    }
}

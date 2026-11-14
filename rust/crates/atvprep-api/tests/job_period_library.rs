//! The period (`from`/`to`, on when a job was queued) and the library (`root_id`) of the job list,
//! and the library counter of `/jobs/counts`.
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
    conn.raw()
        .execute_batch(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Movies','/movies'),(2,'Shows','/shows'),(3,'Empty','/empty');
             INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
               (1,1,'a.mkv','video','.mkv',1,1,'f1','present','clean','idle','none'),
               (2,1,'b.mkv','video','.mkv',1,1,'f2','present','clean','idle','none'),
               (4,2,'d.mkv','video','.mkv',1,1,'f4','present','clean','idle','none'),
               (5,2,'e.mkv','video','.mkv',1,1,'f5','present','clean','idle','none');
             INSERT INTO jobs(id,action,status,requested_at) VALUES
               (1,'verify','succeeded','2026-11-12 08:00:00'),
               (2,'verify','failed','2026-11-13T23:30:00+00:00'),
               (3,'verify','succeeded','2026-11-14 00:15:00.000000'),
               (4,'verify','queued','2026-11-14T09:00:00Z');
             INSERT INTO job_items(job_id,media_file_id,action,position,status) VALUES
               (1,1,'verify',0,'succeeded'),(2,4,'verify',0,'failed'),
               (3,2,'verify',0,'succeeded'),(3,5,'verify',1,'succeeded'),(4,5,'verify',0,'queued');",
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
        "root_id=nonsense",
    ] {
        for path in ["/api/v1/jobs", "/api/v1/jobs/counts"] {
            let (status, body) = api.call(&format!("{path}?{query}")).await;
            assert_eq!(status, 422, "{path}?{query}: {body}");
        }
    }
}

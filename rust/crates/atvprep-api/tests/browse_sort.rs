//! The column view (`/browse`) takes the same order as the file list: `sort` and `dir`, checked by the
//! same rules. Files are ordered like `/files`, folders and libraries by a value over the files below
//! them, and folders always come before files.
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
        let body = to_bytes(response.into_body(), 1 << 22).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> Value {
        let (status, body) = self.request(path).await;
        assert_eq!(status, 200, "{path}: {body}");
        body
    }

    /// Folder names and file names of one level, in the order the column shows them.
    async fn level(&self, query: &str) -> (Vec<String>, Vec<String>) {
        let body = self.get(&format!("/api/v1/browse?{query}")).await;
        let names = |key: &str, field: &str| -> Vec<String> {
            body["data"][key]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry[field].as_str().unwrap().to_owned())
                .collect()
        };
        (names("folders", "name"), names("files", "display_path"))
    }

    async fn folders(&self, query: &str) -> Vec<String> {
        self.level(query).await.0
    }

    async fn files(&self, query: &str) -> Vec<String> {
        self.level(query).await.1
    }
}

fn root(id: i64) -> String {
    Opaque::new(SECRET.as_bytes()).encode("r", id)
}

fn api_with(sql: &str) -> Api {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch(sql).unwrap();
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

/// Alpha (1):
///   Zed/one.mkv     id 1, size 5,   blocker
///   mid/two.mkv     id 2, size 10,  ready
///   big/three.mkv   id 3, size 100, warning
///   mid/four.mkv    id 4, size 20,  ready
///   b.mkv           id 5, size 50,  ready
///   A.mkv           id 6, size 7,   warning
///   c.mkv           id 7, size 7,   not verified
/// beta (2): x.mkv   id 8, size 1000, ready
/// Empty (3) has no files.
fn api() -> Api {
    api_with(
        "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Alpha','/alpha'),(2,'beta','/beta'),(3,'Empty','/empty');
         INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
           (1,1,'Zed/one.mkv','video','.mkv',5,1,'f1','present','blocker','idle','none'),
           (2,1,'mid/two.mkv','video','.mkv',10,1,'f2','present','clean','idle','none'),
           (3,1,'big/three.mkv','video','.mkv',100,1,'f3','present','warning','idle','none'),
           (4,1,'mid/four.mkv','video','.mkv',20,1,'f4','present','clean','idle','none'),
           (5,1,'b.mkv','video','.mkv',50,1,'f5','present','clean','idle','none'),
           (6,1,'A.mkv','video','.mkv',7,1,'f6','present','warning','idle','none'),
           (7,1,'c.mkv','video','.mkv',7,1,'f7','present','unknown','idle','none'),
           (8,2,'x.mkv','video','.mkv',1000,1,'f8','present','clean','idle','none');",
    )
}

#[tokio::test]
async fn files_in_a_column_follow_the_chosen_order() {
    let api = api();
    let alpha = format!("root_id={}", root(1));
    let files = |order: &str| format!("{alpha}&{order}");
    assert_eq!(
        api.files(&files("sort=title&dir=asc")).await,
        ["A.mkv", "b.mkv", "c.mkv"]
    );
    assert_eq!(
        api.files(&files("sort=title&dir=desc")).await,
        ["c.mkv", "b.mkv", "A.mkv"]
    );
    assert_eq!(
        api.files(&files("sort=status&dir=asc")).await,
        ["A.mkv", "c.mkv", "b.mkv"]
    );
    assert_eq!(
        api.files(&files("sort=status&dir=desc")).await,
        ["b.mkv", "c.mkv", "A.mkv"]
    );
    // A tie in size goes by the path.
    assert_eq!(
        api.files(&files("sort=size&dir=asc")).await,
        ["A.mkv", "c.mkv", "b.mkv"]
    );
    assert_eq!(
        api.files(&files("sort=size&dir=desc")).await,
        ["b.mkv", "A.mkv", "c.mkv"]
    );
    // Without a sort the order is recently added, like the list.
    assert_eq!(api.files(&alpha).await, ["b.mkv", "A.mkv", "c.mkv"]);
    assert_eq!(
        api.files(&files("dir=desc")).await,
        ["c.mkv", "A.mkv", "b.mkv"]
    );
}

#[tokio::test]
async fn folders_go_by_a_value_over_the_files_below_them() {
    let api = api();
    let alpha = format!("root_id={}", root(1));
    let folders = |order: &str| format!("{alpha}&{order}");
    assert_eq!(
        api.folders(&folders("sort=title&dir=asc")).await,
        ["big", "mid", "Zed"]
    );
    assert_eq!(
        api.folders(&folders("sort=title&dir=desc")).await,
        ["Zed", "mid", "big"]
    );
    // The worst state below: a blocker first, ready last.
    assert_eq!(
        api.folders(&folders("sort=status&dir=asc")).await,
        ["Zed", "big", "mid"]
    );
    // The sum of the sizes: mid holds 10 + 20.
    assert_eq!(
        api.folders(&folders("sort=size&dir=asc")).await,
        ["Zed", "mid", "big"]
    );
    assert_eq!(
        api.folders(&folders("sort=size&dir=desc")).await,
        ["big", "mid", "Zed"]
    );
    // Recently added: the newest file below (mid has 4, big 3, Zed 1).
    assert_eq!(api.folders(&alpha).await, ["Zed", "big", "mid"]);
    assert_eq!(
        api.folders(&folders("dir=desc")).await,
        ["mid", "big", "Zed"]
    );
}

#[tokio::test]
async fn a_tie_between_folders_goes_by_the_name() {
    let api = api_with(
        "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Alpha','/alpha');
         INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
           (1,1,'b/one.mkv','video','.mkv',5,1,'f1','present','clean','idle','none'),
           (2,1,'a/two.mkv','video','.mkv',5,1,'f2','present','clean','idle','none');",
    );
    let alpha = format!("root_id={}", root(1));
    for order in [
        "sort=size&dir=asc",
        "sort=size&dir=desc",
        "sort=status&dir=desc",
    ] {
        assert_eq!(
            api.folders(&format!("{alpha}&{order}")).await,
            ["a", "b"],
            "{order}"
        );
    }
}

#[tokio::test]
async fn libraries_in_the_first_column_follow_the_same_rule() {
    let api = api();
    assert_eq!(
        api.folders("sort=title&dir=asc").await,
        ["Alpha", "beta", "Empty"]
    );
    assert_eq!(
        api.folders("sort=size&dir=asc").await,
        ["Empty", "Alpha", "beta"]
    );
    assert_eq!(
        api.folders("sort=status&dir=asc").await,
        ["Alpha", "beta", "Empty"]
    );
    assert_eq!(api.folders("dir=desc").await, ["beta", "Alpha", "Empty"]);
}

#[tokio::test]
async fn every_folder_carries_the_size_of_the_files_below_it() {
    let api = api();
    let top = api.get("/api/v1/browse?sort=title&dir=asc").await;
    let sizes: Vec<&Value> = top["data"]["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|folder| &folder["size"])
        .collect();
    assert_eq!(sizes, [&json!(199), &json!(1000), &json!(0)]);
    // The other fields stay as they were.
    assert_eq!(top["data"]["folders"][0]["total"], 7);
    assert_eq!(top["data"]["folders"][0]["id"], root(1));

    let level = api
        .get(&format!(
            "/api/v1/browse?root_id={}&sort=title&dir=asc",
            root(1)
        ))
        .await;
    assert_eq!(
        level["data"]["folders"],
        json!([
            {"name": "big", "id": null, "counts": {"warning": 1}, "total": 1, "size": 100},
            {"name": "mid", "id": null, "counts": {"clean": 2}, "total": 2, "size": 30},
            {"name": "Zed", "id": null, "counts": {"blocker": 1}, "total": 1, "size": 5},
        ])
    );
}

#[tokio::test]
async fn bad_sort_values_get_the_same_answer_as_the_list() {
    let api = api();
    for query in ["sort=name", "dir=up", "sort=size&dir=sideways"] {
        let (list_status, list) = api.request(&format!("/api/v1/files?{query}")).await;
        let (status, body) = api.request(&format!("/api/v1/browse?{query}")).await;
        assert_eq!(status, list_status, "{query}");
        assert!((400..500).contains(&status), "{query}: {status}");
        assert_eq!(body["error"], list["error"], "{query}");
        assert_eq!(body["detail"], list["detail"], "{query}");
        let (below, _) = api
            .request(&format!("/api/v1/browse/below?root_id={}&{query}", root(1)))
            .await;
        assert_eq!(below, list_status, "{query}");
    }
}

#[tokio::test]
async fn the_limit_of_500_files_applies_after_sorting() {
    // 501 files in one folder; the largest was added first, so the cut must come after the sort.
    let api = api_with(
        "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Alpha','/alpha');
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<501)
         INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action)
           SELECT i,1,printf('f%03d.mkv',i),'video','.mkv',1000-i,1,'f'||i,'present','clean','idle','none' FROM n;",
    );
    let alpha = root(1);
    let body = api
        .get(&format!("/api/v1/browse?root_id={alpha}&sort=size&dir=asc"))
        .await;
    // The response bound may cut sooner than 500; what is listed is still the start of the order.
    let files: Vec<&str> = body["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["display_path"].as_str().unwrap())
        .collect();
    assert!(!files.is_empty() && files.len() <= 500);
    assert_eq!(body["data"]["truncated"], true);
    // Smallest first: the last added (f501) leads, counting down from there.
    let expected: Vec<String> = (0..files.len())
        .map(|n| format!("f{:03}.mkv", 501 - n))
        .collect();
    assert_eq!(files, expected);

    let newest = api
        .get(&format!("/api/v1/browse?root_id={alpha}&dir=desc"))
        .await;
    assert_eq!(newest["data"]["files"][0]["display_path"], "f501.mkv");
    let by_name = api
        .get(&format!(
            "/api/v1/browse?root_id={alpha}&sort=title&dir=desc"
        ))
        .await;
    assert_eq!(by_name["data"]["files"][0]["display_path"], "f501.mkv");
}

#[tokio::test]
async fn the_files_below_a_folder_are_cut_after_sorting_too() {
    let api = api_with(
        "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'Alpha','/alpha');
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<501)
         INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action)
           SELECT i,1,printf('f%03d.mkv',i),'video','.mkv',1000-i,1,'f'||i,'present','clean','idle','none' FROM n;",
    );
    let body = api
        .get(&format!(
            "/api/v1/browse/below?root_id={}&sort=size&dir=desc",
            root(1)
        ))
        .await;
    let first = body["data"]["files"][0]["display_path"].as_str().unwrap();
    assert_eq!(first, "f001.mkv");
    assert_eq!(body["data"]["truncated"], true);
}

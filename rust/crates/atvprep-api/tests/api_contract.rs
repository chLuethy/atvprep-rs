//! Reusable cross-language request/response steps and one shared historical seed.
mod support;
use atvprep_api::opaque::Opaque;
use atvprep_manager::{db::Db, schema};
use serde_json::{Value, json};

#[tokio::test]
async fn contract_replays_recorded_python_steps() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../testdata/golden/api_contract.json")).unwrap();
    let mut contract = support::Contract::new();
    for case in fixture["cases"].as_array().unwrap() {
        contract.replay(case).await;
    }
}

#[test]
fn the_harness_normalises_ids_timestamps_and_tokens() {
    let opaque = Opaque::new("fixture-secret");
    let mut value = json!({"id":opaque.encode("m",42),"again":opaque.encode("m",42),"requested_at":"2026-01-02T12:30:00Z","csrf_token":"private-value"});
    support::normalise::value(&mut value, "", &opaque);
    assert_eq!(
        value,
        json!({"id":"<m:42>","again":"<m:42>","requested_at":"<ts>","csrf_token":"<csrf>"})
    );
    let path = support::normalise::render("/api/v1/files/<m:42>?root_id=<r:1>", &opaque);
    assert_eq!(
        path,
        format!(
            "/api/v1/files/{}?root_id={}",
            opaque.encode("m", 42),
            opaque.encode("r", 1)
        )
    );
    assert_eq!(
        support::normalise::cookie("session=private; Path=/; HttpOnly"),
        "<cookie>; Path=/; HttpOnly"
    );
}

#[test]
fn seed_applies_and_reads_back() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    support::seed::apply(conn.raw());
    for table in [
        "library_roots",
        "media_files",
        "sidecars",
        "inspections",
        "jobs",
        "job_items",
        "events",
        "runtime_settings",
        "schedules",
    ] {
        let count: i64 = conn
            .raw()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "{table}");
    }
    let file: (String, i64) = conn
        .raw()
        .query_row(
            "SELECT relative_path,size FROM media_files WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(file, ("Movie/film.mp4".to_owned(), 123));
    let mut statement = conn.raw().prepare("PRAGMA foreign_key_check").unwrap();
    assert!(statement.query([]).unwrap().next().unwrap().is_none());
}

#[tokio::test]
async fn auth_routes_replay_recorded_python_steps() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../testdata/golden/api_auth_contract.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        support::Contract::new().replay(case).await;
    }
}

#[tokio::test]
async fn mutation_pipeline_replays_recorded_python_steps() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../testdata/golden/api_mutation_contract.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        support::Contract::new().replay(case).await;
    }
}

#[tokio::test]
async fn read_edges_replay_recorded_python_steps() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../testdata/golden/api_read_edges.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        support::Contract::new().replay(case).await;
    }
}

#[tokio::test]
async fn job_writes_replay_recorded_python_steps() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../testdata/golden/api_jobs_contract.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut contract = support::Contract::with_seed(case["input"]["seed"].as_str().unwrap());
        let retry = case["input"]["setup"] == "prepare-retry";
        if retry {
            contract.seed_failed_prepare_and_change_approver();
        } else {
            contract.assert_missing_media_is_rejected_atomically().await;
        }
        contract.replay(case).await;
        contract.assert_prepare_approvals(if retry { 1 } else { 3 });
    }
}

#[tokio::test]
async fn settings_replay_recorded_python_steps() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../testdata/golden/api_settings_contract.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        support::Contract::new().replay(case).await;
    }
}

#[tokio::test]
async fn parallel_http_settings_writers_have_one_revision_audit_and_csrf_winner() {
    for file_based in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let db = if file_based {
            Db::open(&directory.path().join("settings.db")).unwrap()
        } else {
            Db::memory().unwrap()
        };
        support::Contract::with_database(db, "seed")
            .assert_parallel_settings_have_one_winner()
            .await;
    }
}

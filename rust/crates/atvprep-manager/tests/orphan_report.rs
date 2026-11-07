//! Verwaiste Teilüberarbeitungen: Meldung am Scan und Entfernen von Hand.
use std::{fs, path::Path};

use atvprep_manager::{
    clock::SystemClock,
    db::{Conn, Db},
    repo::Ctx,
    schema,
    worker::orphans::{self, RemoveOutcome},
};

fn setup(root: &Path) -> (Db, Conn) {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw()
        .execute(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'lib',?1)",
            [root.canonicalize().unwrap().to_str().unwrap()],
        )
        .unwrap();
    (db, conn)
}

fn ctx(conn: &Conn) -> Ctx<'_> {
    Ctx {
        conn: conn.raw(),
        clock: &SystemClock,
    }
}

fn leftover(dir: &Path, pid: u32, stem: &str, bytes: usize) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join(format!(".tmp_fixmeta_enc.{stem}.{pid}-0a1b2c3d.mp4")),
        vec![b'x'; bytes],
    )
    .unwrap();
}

fn events(conn: &Conn, topic: &str) -> Vec<String> {
    let mut statement = conn
        .raw()
        .prepare("SELECT payload_json FROM events WHERE topic=?1 ORDER BY id")
        .unwrap();
    statement
        .query_map([topic], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn add_running_item(conn: &Conn, relative: &str) {
    conn.raw()
        .execute_batch(&format!(
            "INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action)
             VALUES(1,1,'{relative}','video','.mkv',1,1,'x','present','unknown','running','fix');
             INSERT INTO jobs(id,action,status) VALUES(1,'fix','running');
             INSERT INTO job_items(job_id,media_file_id,action,position,status) VALUES(1,1,'fix',1,'running');"
        ))
        .unwrap();
}

#[test]
fn find_lists_young_and_live_leftovers_with_path_and_size() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    leftover(&root.path().join("Show"), std::process::id(), "Film", 5);
    fs::write(root.path().join("Show/Film.mkv"), "source").unwrap();

    let found = orphans::find(ctx(&conn), 1).unwrap();

    assert_eq!(found.len(), 1);
    assert!(
        found[0]
            .relative_path
            .starts_with("Show/.tmp_fixmeta_enc.Film.")
    );
    assert_eq!(found[0].size, 5);
}

#[test]
fn find_skips_similar_names_and_the_directory_of_a_running_item() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    fs::write(root.path().join("Film.TMP.mp4"), "x").unwrap();
    fs::write(
        root.path().join(".tmp_fixmeta_enc.Film.1-0A1B2C3D.mp4"),
        "x",
    )
    .unwrap();
    leftover(&root.path().join("Busy"), 4711, "Film", 1);
    add_running_item(&conn, "Busy/Film.mkv");
    // Ein Unterverzeichnis ist ein anderes Verzeichnis: die Sperre gilt nur für das gleiche.
    leftover(&root.path().join("Busy/Sub"), 4711, "Film", 1);

    let found = orphans::find(ctx(&conn), 1).unwrap();

    assert_eq!(found.len(), 1);
    assert!(found[0].relative_path.starts_with("Busy/Sub/"), "{found:?}");
}

#[test]
fn remove_deletes_what_was_confirmed_and_writes_history() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    leftover(&root.path().join("A"), std::process::id(), "One", 3);
    leftover(&root.path().join("B"), 4711, "Two", 4);
    let source = root.path().join("A/One.mkv");
    fs::write(&source, "source").unwrap();

    let outcome = orphans::remove(ctx(&conn), 1, 2, 7).unwrap();

    assert_eq!(outcome, RemoveOutcome::Removed { count: 2, bytes: 7 });
    assert!(orphans::find(ctx(&conn), 1).unwrap().is_empty());
    assert_eq!(fs::read_to_string(source).unwrap(), "source");
    let per_file = events(&conn, "orphan.removed");
    assert_eq!(per_file.len(), 2);
    assert!(per_file[0].contains("\"manual\""), "{}", per_file[0]);
    let summary = events(&conn, "orphan.cleanup");
    assert_eq!(summary.len(), 1);
    assert!(summary[0].contains("\"count\":2"), "{}", summary[0]);
}

#[test]
fn remove_refuses_and_reports_fresh_numbers_when_the_set_changed() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    leftover(&root.path().join("A"), 4711, "One", 3);

    let outcome = orphans::remove(ctx(&conn), 1, 2, 9).unwrap();

    assert_eq!(outcome, RemoveOutcome::Changed { count: 1, bytes: 3 });
    assert_eq!(orphans::find(ctx(&conn), 1).unwrap().len(), 1);
    assert!(events(&conn, "orphan.removed").is_empty());
}

#[test]
fn remove_leaves_files_in_the_directory_of_a_running_item() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    leftover(&root.path().join("Busy"), 4711, "Film", 2);
    add_running_item(&conn, "Busy/Film.mkv");

    let outcome = orphans::remove(ctx(&conn), 1, 0, 0).unwrap();

    assert_eq!(outcome, RemoveOutcome::Removed { count: 0, bytes: 0 });
    assert!(fs::read_dir(root.path().join("Busy")).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp_fixmeta_")
    }));
}

#[test]
fn symlinks_are_neither_listed_nor_followed() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    leftover(outside.path(), 4711, "Out", 1);
    let target = fs::read_dir(outside.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::os::unix::fs::symlink(&target, root.path().join(target.file_name().unwrap())).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("Link")).unwrap();

    assert!(orphans::find(ctx(&conn), 1).unwrap().is_empty());
    orphans::remove(ctx(&conn), 1, 0, 0).unwrap();
    assert!(target.exists());
}

#[test]
fn the_scan_report_stores_count_bytes_and_a_list_on_the_scan() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    leftover(&root.path().join("A"), 4711, "One", 3);
    conn.raw()
        .execute(
            "INSERT INTO scan_runs(id,root_id,generation,status) VALUES(1,1,1,'succeeded')",
            [],
        )
        .unwrap();

    orphans::report_for_scan(ctx(&conn), 1, 1).unwrap();

    let stored: String = conn
        .raw()
        .query_row("SELECT orphans_json FROM scan_runs WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(value["count"], 1);
    assert_eq!(value["bytes"], 3);
    assert_eq!(value["files"].as_array().unwrap().len(), 1);
}

#[test]
fn a_scan_without_leftovers_stores_no_report() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    conn.raw()
        .execute(
            "INSERT INTO scan_runs(id,root_id,generation,status) VALUES(1,1,1,'succeeded')",
            [],
        )
        .unwrap();
    orphans::report_for_scan(ctx(&conn), 1, 1).unwrap();
    let stored: Option<String> = conn
        .raw()
        .query_row("SELECT orphans_json FROM scan_runs WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(stored, None);
}

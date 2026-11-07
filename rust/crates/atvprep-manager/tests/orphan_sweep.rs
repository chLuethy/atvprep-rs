//! The start-up sweep deletes only orphaned partial rewrites and leaves everything else alone.
use std::{
    fs::{self, File},
    path::Path,
    time::{Duration, SystemTime},
};

use atvprep_core::prepare::paths::TEMP_PREFIX;
use atvprep_manager::{
    clock::SystemClock,
    db::{Conn, Db},
    repo::Ctx,
    schema,
    worker::orphans::sweep,
};

fn setup(root: &Path) -> (Db, Conn) {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw()
        .execute(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'lib',?1)",
            [root.to_str().unwrap()],
        )
        .unwrap();
    (db, conn)
}

fn aged(path: &Path, minutes: u64) {
    let file = File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(minutes * 60))
        .unwrap();
}

fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("0")
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn temp(dir: &Path, pid: u32, stem: &str) -> std::path::PathBuf {
    let path = dir.join(format!("{TEMP_PREFIX}enc.{stem}.{pid}-0a1b2c3d.mp4"));
    fs::write(&path, "half").unwrap();
    path
}

fn run(conn: &Conn) -> atvprep_manager::worker::orphans::OrphanSweep {
    sweep(Ctx {
        conn: conn.raw(),
        clock: &SystemClock,
    })
    .unwrap()
}

fn history(conn: &Conn) -> Vec<String> {
    let mut statement = conn
        .raw()
        .prepare("SELECT payload_json FROM events WHERE topic='orphan.removed' ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn add_running_item(conn: &Conn, relative: &str, status: &str) {
    conn.raw()
        .execute_batch(&format!(
            "INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action)
             VALUES(1,1,'{relative}','video','.mkv',1,1,'x','present','unknown','running','fix');
             INSERT INTO jobs(id,action,status) VALUES(1,'fix','running');
             INSERT INTO job_items(job_id,media_file_id,action,position,status) VALUES(1,1,'fix',1,'{status}');"
        ))
        .unwrap();
}

#[test]
fn an_old_leftover_without_item_or_live_pid_is_removed_and_recorded_while_the_source_stays() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    fs::create_dir(root.path().join("Show")).unwrap();
    let source = root.path().join("Show/Film.mkv");
    fs::write(&source, "source").unwrap();
    let leftover = temp(&root.path().join("Show"), dead_pid(), "Film");
    aged(&leftover, 11);

    let outcome = run(&conn);

    assert!(outcome.ran);
    assert_eq!(outcome.removed, 1);
    assert!(!leftover.exists());
    assert_eq!(fs::read_to_string(&source).unwrap(), "source");
    let events = history(&conn);
    assert_eq!(events.len(), 1);
    assert!(events[0].contains("Show/"), "{}", events[0]);
}

#[test]
fn a_young_leftover_stays() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    let leftover = temp(root.path(), dead_pid(), "Film");
    aged(&leftover, 9);
    assert_eq!(run(&conn).removed, 0);
    assert!(leftover.exists());
    assert!(history(&conn).is_empty());
}

#[test]
fn a_leftover_of_a_living_pid_stays() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    let leftover = temp(root.path(), std::process::id(), "Film");
    aged(&leftover, 60);
    assert_eq!(run(&conn).removed, 0);
    assert!(leftover.exists());
}

#[test]
fn a_leftover_in_a_directory_with_a_running_item_stays() {
    for status in ["claimed", "running"] {
        let root = tempfile::tempdir().unwrap();
        let (_db, conn) = setup(root.path());
        fs::create_dir(root.path().join("Show")).unwrap();
        add_running_item(&conn, "Show/Film.mkv", status);
        let leftover = temp(&root.path().join("Show"), dead_pid(), "Film");
        aged(&leftover, 60);
        assert_eq!(run(&conn).removed, 0, "{status}");
        assert!(leftover.exists(), "{status}");
    }
}

#[test]
fn a_running_item_elsewhere_does_not_protect_this_directory() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    fs::create_dir(root.path().join("Show")).unwrap();
    fs::create_dir(root.path().join("Other")).unwrap();
    add_running_item(&conn, "Other/Film.mkv", "running");
    let leftover = temp(&root.path().join("Show"), dead_pid(), "Film");
    aged(&leftover, 60);
    assert_eq!(run(&conn).removed, 1);
    assert!(!leftover.exists());
}

#[test]
fn similarly_named_files_stay() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    let pid = dead_pid();
    let names = [
        "Film.TMP.mkv".to_owned(),
        format!("Film.mp4.tmp_moov.{pid}-0a1b2c3d"),
        format!("{TEMP_PREFIX}enc.Film.{pid}-0a1b2c3d.mkv"),
        format!("tmp_fixmeta_enc.Film.{pid}-0a1b2c3d.mp4"),
    ];
    for name in &names {
        let path = root.path().join(name);
        fs::write(&path, "x").unwrap();
        aged(&path, 60);
    }
    assert_eq!(run(&conn).removed, 0);
    for name in &names {
        assert!(root.path().join(name).exists(), "{name}");
    }
}

#[test]
fn nothing_is_removed_while_the_gate_is_closed() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    conn.raw()
        .execute(
            "UPDATE deployment_gate SET mode='maintenance', generation=generation+1",
            [],
        )
        .unwrap();
    let leftover = temp(root.path(), dead_pid(), "Film");
    aged(&leftover, 60);
    let outcome = run(&conn);
    assert!(!outcome.ran);
    assert_eq!(outcome.removed, 0);
    assert!(leftover.exists());
}

#[test]
fn the_sweep_continues_when_an_event_cannot_be_written() {
    let root = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    let first = temp(root.path(), dead_pid(), "One");
    let second = temp(root.path(), dead_pid(), "Two");
    aged(&first, 60);
    aged(&second, 60);
    conn.raw()
        .execute_batch(
            "CREATE TRIGGER no_events BEFORE INSERT ON events
             BEGIN SELECT RAISE(ABORT, 'events are read-only'); END;",
        )
        .unwrap();

    let outcome = run(&conn);

    assert_eq!(outcome.removed, 2);
    assert!(!first.exists());
    assert!(!second.exists());
}

#[test]
fn a_symlink_with_the_working_file_name_is_neither_followed_nor_removed() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let (_db, conn) = setup(root.path());
    let target = temp(outside.path(), dead_pid(), "Out");
    aged(&target, 60);
    let link = root.path().join(target.file_name().unwrap());
    std::os::unix::fs::symlink(&target, &link).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("Dir")).unwrap();

    assert_eq!(run(&conn).removed, 0);

    assert!(target.exists());
    assert!(link.symlink_metadata().is_ok());
}

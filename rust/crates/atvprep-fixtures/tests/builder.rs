//! The builder's promise: what it lays down satisfies the invariants of real data.

use atvprep_fixtures::{Fixtures, Verdict};
use atvprep_manager::clock::SystemClock;
use atvprep_manager::db::{Conn, Db};
use atvprep_manager::media::Media;
use atvprep_manager::models::MediaFile;
use atvprep_manager::repo::Ctx;
use atvprep_manager::schema;

fn migrated() -> Conn {
    let mut conn = Db::memory().unwrap().conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn
}

fn stored(conn: &Conn, id: i64) -> MediaFile {
    Media(Ctx {
        conn: conn.raw(),
        clock: &SystemClock,
    })
    .get(id)
    .unwrap()
    .unwrap()
}

#[test]
fn a_library_with_files_returns_their_ids_in_order() {
    let conn = migrated();
    let fx = Fixtures::new(conn.raw());
    let root = fx.library("Movies");
    let first = fx.file(root, "a.mp4").create();
    let second = fx.file(root, "b.mp4").create();
    assert_eq!((root, first, second), (1, 1, 2));
    assert_eq!(stored(&conn, first).relative_path, "a.mp4");
}

#[test]
fn a_file_with_a_warning_has_the_warning_state_and_its_reason() {
    let conn = migrated();
    let fx = Fixtures::new(conn.raw());
    let root = fx.library("Movies");
    let id = fx.file(root, "a.mp4").verdict(Verdict::Warning).create();
    let file = stored(&conn, id);
    assert_eq!(file.compliance_state, "warning");
    assert!(file.compliance_reasons[0].starts_with("[warning]"));
}

#[test]
fn a_new_file_is_unknown_until_a_verdict_is_given() {
    let conn = migrated();
    let fx = Fixtures::new(conn.raw());
    let root = fx.library("Movies");
    let id = fx.file(root, "a.mp4").create();
    assert_eq!(stored(&conn, id).compliance_state, "unknown");
}

#[test]
fn a_job_has_one_queued_item_per_file() {
    let conn = migrated();
    let fx = Fixtures::new(conn.raw());
    let root = fx.library("Movies");
    let a = fx.file(root, "a.mp4").create();
    let b = fx.file(root, "b.mp4").create();
    let job = fx.job("verify", &[a, b]).create();
    let items: i64 = conn
        .raw()
        .query_row(
            "SELECT count(*) FROM job_items WHERE job_id=?1 AND status='queued'",
            [job],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((job, items), (1, 2));
}

#[test]
fn a_file_can_want_a_fix_or_have_failed_inspection() {
    let conn = migrated();
    let fx = Fixtures::new(conn.raw());
    let root = fx.library("Movies");
    let wanted = fx.file(root, "a.mp4").desired_action("fix").create();
    let broken = fx.file(root, "b.mp4").inspection_failed().create();
    assert_eq!(stored(&conn, wanted).desired_action, "fix");
    assert_eq!(stored(&conn, broken).compliance_state, "probe_failed");
}

#[test]
fn a_job_can_end_partly_with_failed_items_and_be_cleared() {
    let conn = migrated();
    let fx = Fixtures::new(conn.raw());
    let root = fx.library("Movies");
    let a = fx.file(root, "a.mp4").create();
    let job = fx
        .job("verify", &[a])
        .status("partial_success")
        .item_status("failed")
        .cleared_at("2026-01-01 00:00:00")
        .create();
    let row: (String, String, Option<String>) = conn
        .raw()
        .query_row(
            "SELECT j.status, i.status, j.cleared_at FROM jobs j JOIN job_items i ON i.job_id=j.id WHERE j.id=?1",
            [job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "partial_success".to_owned(),
            "failed".to_owned(),
            Some("2026-01-01 00:00:00".to_owned())
        )
    );
}

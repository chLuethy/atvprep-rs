//! The count a schedule shows and the files a run queues come from one selection.
use atvprep_fixtures::{Fixtures, Verdict};
use atvprep_manager::{
    clock::FixedClock,
    db::{Conn, Db},
    repo::Ctx,
    schedule_candidates::{ScheduleSelection, candidate_count, candidate_ids},
    schedules::run_schedule,
    schema,
};

const NOW: i64 = 1_700_000_000_000_000;

/// Library 1 with MP4s of every need, an mkv, an mkv with a converted sibling, a file with an open
/// fix item and a missing file; library 2 holds one more MP4.
fn seed(db: &Db) -> Conn {
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    {
        let fx = Fixtures::new(conn.raw());
        let a = fx.library_at("a", "/a");
        let b = fx.library_at("b", "/b");
        let file = |root: i64, name: &str, verdict: Verdict, need: &'static str| {
            fx.file(root, name)
                .fingerprint(name)
                .verdict(verdict)
                .encode(need, "none")
        };
        let _ = file(a, "none.mp4", Verdict::Warning, "none").create();
        let _ = file(a, "audio.mp4", Verdict::Warning, "audio").create();
        let _ = file(a, "video.m4v", Verdict::Blocker, "video").create();
        let _ = file(a, "clean.mp4", Verdict::Clean, "none").create();
        let _ = file(a, "old.mkv", Verdict::Warning, "none").create();
        let _ = file(a, "done.mkv", Verdict::Warning, "none").create();
        let _ = file(a, "done.mp4", Verdict::Clean, "none").create();
        let busy = file(a, "busy.mp4", Verdict::Warning, "none").create();
        let _ = file(a, "gone.mp4", Verdict::Warning, "none")
            .missing()
            .create();
        let _ = file(b, "other.mp4", Verdict::Warning, "none").create();
        let _ = fx.job("fix", &[busy]).status("running").create();
    }
    conn
}

fn schedule(
    conn: &Conn,
    action: &str,
    root: Option<i64>,
    compliance: Option<&str>,
    encode: &str,
    max: i64,
) {
    let fx = Fixtures::new(conn.raw());
    let mut schedule = fx
        .schedule("t", action)
        .days_mask(127)
        .time_of_day("03:00")
        .max_files(max)
        .enabled(false)
        .armed_at("2000-01-01")
        .encode(encode)
        .approved(1, 1_700_000_000, &"a".repeat(64));
    if let Some(root) = root {
        schedule = schedule.library(root);
    }
    if let Some(compliance) = compliance {
        schedule = schedule.compliance_state(compliance);
    }
    let _ = schedule.create();
}

fn queued(conn: &Conn) -> Vec<i64> {
    let mut statement = conn
        .raw()
        .prepare("SELECT media_file_id FROM job_items WHERE job_id>1 ORDER BY position")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn the_shown_count_is_the_size_of_the_queued_set_for_every_action() {
    let cases: Vec<(&str, Option<i64>, Option<&str>, &str)> = vec![
        ("verify", None, None, "copy"),
        ("verify", Some(1), None, "copy"),
        ("verify", None, Some("warning"), "copy"),
        ("prepare", None, None, "copy"),
        ("prepare", None, None, "audio"),
        ("prepare", None, None, "all"),
        ("prepare", Some(1), Some("warning"), "all"),
    ];
    for (action, root, compliance, encode) in cases {
        for max in [1, 2, 500] {
            let db = Db::memory().unwrap();
            let conn = seed(&db);
            schedule(&conn, action, root, compliance, encode, max);
            let selection = ScheduleSelection::load(conn.raw(), 1).unwrap().unwrap();
            let shown = candidate_count(conn.raw(), &selection, Some(max)).unwrap();
            let ids = candidate_ids(conn.raw(), &selection, Some(max)).unwrap();
            let clock = FixedClock(NOW);
            run_schedule(
                Ctx {
                    conn: conn.raw(),
                    clock: &clock,
                },
                1,
            )
            .unwrap();
            let label = format!("{action} {root:?} {compliance:?} {encode} max {max}");
            assert_eq!(usize::try_from(shown).unwrap(), ids.len(), "{label}");
            let mut queued_ids = queued(&conn);
            let mut expected = ids.clone();
            queued_ids.sort_unstable();
            expected.sort_unstable();
            assert_eq!(queued_ids, expected, "{label}");
            assert!(i64::try_from(ids.len()).unwrap() <= max, "{label}");
        }
    }
}

#[test]
fn without_a_limit_the_count_covers_every_candidate() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    schedule(&conn, "verify", None, None, "copy", 1);
    let selection = ScheduleSelection::load(conn.raw(), 1).unwrap().unwrap();
    // Present MP4-likes without an open fix item: 1, 2, 3, 4, 7, 10.
    assert_eq!(candidate_count(conn.raw(), &selection, None).unwrap(), 6);
    assert_eq!(candidate_count(conn.raw(), &selection, Some(1)).unwrap(), 1);
    assert_eq!(
        candidate_ids(conn.raw(), &selection, None).unwrap().len(),
        6
    );
}

#[test]
fn preparing_skips_converted_originals_and_clean_mp4s() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    schedule(&conn, "prepare", Some(1), None, "all", 500);
    let selection = ScheduleSelection::load(conn.raw(), 1).unwrap().unwrap();
    // 4 and 7 are clean, 6 has a converted sibling (7), 8 is busy, 9 is missing, 10 is elsewhere.
    assert_eq!(
        candidate_ids(conn.raw(), &selection, None).unwrap(),
        vec![1, 2, 3, 5]
    );
}

//! The count a schedule shows and the files a run queues come from one selection.
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
    conn.raw()
        .execute_batch(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'a','/a'),(2,'b','/b');
             INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action,encode_need) VALUES
             (1,1,'none.mp4','video','.mp4',1,1,'1','present','warning','idle','none','none'),
             (2,1,'audio.mp4','video','.mp4',1,1,'2','present','warning','idle','none','audio'),
             (3,1,'video.m4v','video','.m4v',1,1,'3','present','blocker','idle','none','video'),
             (4,1,'clean.mp4','video','.mp4',1,1,'4','present','clean','idle','none','none'),
             (5,1,'old.mkv','video','.mkv',1,1,'5','present','warning','idle','none','none'),
             (6,1,'done.mkv','video','.mkv',1,1,'6','present','warning','idle','none','none'),
             (7,1,'done.mp4','video','.mp4',1,1,'7','present','clean','idle','none','none'),
             (8,1,'busy.mp4','video','.mp4',1,1,'8','present','warning','idle','none','none'),
             (9,1,'gone.mp4','video','.mp4',1,1,'9','missing','warning','idle','none','none'),
             (10,2,'other.mp4','video','.mp4',1,1,'10','present','warning','idle','none','none');
             INSERT INTO jobs(id,action,status) VALUES(1,'fix','running');
             INSERT INTO job_items(job_id,media_file_id,action,position,status) VALUES(1,8,'fix',0,'running');",
        )
        .unwrap();
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
    conn.raw().execute("DELETE FROM schedules", []).unwrap();
    conn.raw()
        .execute(
            "INSERT INTO schedules(id,name,action,root_id,compliance_state,days_mask,time_of_day,max_files,enabled,armed_at,encode,approved_by,approved_at,approval_mac)
             VALUES(1,'t',?1,?2,?3,127,'03:00',?4,0,'2000-01-01',?5,1,1700000000,?6)",
            rusqlite::params![action, root, compliance, max, encode, "a".repeat(64)],
        )
        .unwrap();
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

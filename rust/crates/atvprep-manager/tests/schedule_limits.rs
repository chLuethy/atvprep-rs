//! A stored schedule that breaks a limit (minimum interval, files per run) is paused, not run.
use atvprep_manager::{
    clock::FixedClock,
    db::{Conn, Db},
    repo::Ctx,
    schedules::run_due_schedules,
    schema,
};
use chrono::{DateTime, Duration, Local, Utc};

const NOW: i64 = 1_700_000_000_000_000;

/// An enabled, approved schedule of `action` that was due 30 minutes before NOW.
fn seed(db: &Db, action: &str, max_files: i64, cron: Option<&str>) -> Conn {
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'test','/library');
    INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
    (1,1,'one.mkv','video','.mkv',1,1,'one','present','warning','idle','none');").unwrap();
    let time = (DateTime::<Utc>::from_timestamp_micros(NOW)
        .unwrap()
        .with_timezone(&Local)
        - Duration::minutes(0))
    .format("%H:%M")
    .to_string();
    conn.raw().execute("INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,cron,max_files,enabled,armed_at,approved_by,approved_at,approval_mac) VALUES(1,'t',?1,1,127,?2,?3,?4,1,'2000-01-01',1,1700000000,?5)",rusqlite::params![action,time,cron,max_files,"a".repeat(64)]).unwrap();
    conn
}

fn state(conn: &Conn) -> (bool, Option<String>, Vec<String>, i64) {
    let (enabled, mac): (bool, Option<String>) = conn
        .raw()
        .query_row("SELECT enabled, approval_mac FROM schedules", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    let mut statement = conn
        .raw()
        .prepare("SELECT result FROM schedule_runs ORDER BY id")
        .unwrap();
    let results = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let jobs = conn
        .raw()
        .query_row("SELECT count(*) FROM jobs", [], |row| row.get(0))
        .unwrap();
    (enabled, mac, results, jobs)
}

fn dispatch(conn: &Conn) -> usize {
    let clock = FixedClock(NOW);
    run_due_schedules(Ctx {
        conn: conn.raw(),
        clock: &clock,
    })
    .unwrap()
}

#[test]
fn an_over_limit_max_files_pauses_the_schedule_and_is_recorded_once() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 13, None);
    assert_eq!(dispatch(&conn), 0);
    let (enabled, mac, results, jobs) = state(&conn);
    assert!(!enabled);
    assert_eq!(mac, None, "the approval ends with the pause");
    assert_eq!(jobs, 0);
    assert_eq!(results.len(), 1);
    assert!(results[0].starts_with("Paused: "), "{results:?}");
    assert!(results[0].contains("12"), "{results:?}");
    // Paused already: further passes add nothing.
    assert_eq!(dispatch(&conn), 0);
    assert_eq!(state(&conn).2.len(), 1);
}

#[test]
fn a_too_frequent_cron_pauses_a_schedule_that_changes_files() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "convert", 3, Some("0,10 * * * *"));
    assert_eq!(dispatch(&conn), 0);
    let (enabled, _, results, jobs) = state(&conn);
    assert!(!enabled);
    assert_eq!(jobs, 0);
    assert!(results[0].starts_with("Paused: "), "{results:?}");
    assert!(results[0].contains("15"), "{results:?}");
}

#[test]
fn schedules_within_the_limits_and_verify_at_any_interval_keep_running() {
    let db = Db::memory().unwrap();
    let conn = seed(&db, "fix", 12, None);
    assert_eq!(dispatch(&conn), 1);
    assert!(state(&conn).0);
    let db = Db::memory().unwrap();
    let conn = seed(&db, "verify", 500, Some("* * * * *"));
    assert_eq!(dispatch(&conn), 1);
    assert!(state(&conn).0);
}

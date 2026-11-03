//! Actual lock collisions during scheduler shutdown, without weakening fencing.
use atvprep_manager::{clock::FixedClock, db::Db, repo::Ctx, scheduler::Scheduler, schema};
use std::time::{Duration, Instant};
#[test]
fn shutdown_retries_transient_locks_and_exhausts_persistent_locks_quietly() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("scheduler.sqlite"))
        .unwrap()
        .with_busy_timeout(Duration::from_millis(1));
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let clock = FixedClock(1_700_000_000_000_000);
    let mut scheduler = Scheduler::new(
        Ctx {
            conn: conn.raw(),
            clock: &clock,
        },
        "worker",
        30,
    );
    assert!(scheduler.acquire(3600).unwrap());
    let blocker = db.conn().unwrap();
    blocker.raw().execute_batch("BEGIN IMMEDIATE").unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        blocker.raw().execute_batch("ROLLBACK").unwrap();
    });
    scheduler.shutdown(5);
    release.join().unwrap();
    assert_eq!(
        conn.raw()
            .query_row("SELECT owner_id FROM scheduler_leases", [], |row| row
                .get::<_, Option<String>>(0))
            .unwrap(),
        None
    );
    assert!(scheduler.acquire(3600).unwrap());
    let blocker = db.conn().unwrap();
    blocker.raw().execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    scheduler.shutdown(2);
    assert!(started.elapsed() < Duration::from_secs(1));
    blocker.raw().execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        conn.raw()
            .query_row("SELECT owner_id FROM scheduler_leases", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "worker"
    );
    scheduler.shutdown(1);
}
#[test]
fn stale_scheduler_shutdown_cannot_release_a_successor_lease() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let clock = FixedClock(1_700_000_000_000_000);
    let later = FixedClock(clock.0 + 31_000_000);
    let mut first = Scheduler::new(
        Ctx {
            conn: conn.raw(),
            clock: &clock,
        },
        "first",
        30,
    );
    assert!(first.acquire(3600).unwrap());
    let mut successor = Scheduler::new(
        Ctx {
            conn: conn.raw(),
            clock: &later,
        },
        "successor",
        30,
    );
    assert!(successor.acquire(3600).unwrap());
    first.shutdown(3);
    assert_eq!(
        conn.raw()
            .query_row("SELECT owner_id FROM scheduler_leases", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "successor"
    );
}

//! Shared-cache lock failures cannot lose a permit or bypass its release.
use atvprep_manager::{
    clock::FixedClock,
    db::{Conn, Db},
    deployment_gate::DeploymentGate,
    repo::Ctx,
    schema,
};
use std::time::Duration;
fn blocker(db: &Db) -> std::thread::JoinHandle<()> {
    let conn: Conn = db.conn().unwrap();
    conn.raw()
        .execute_batch("BEGIN IMMEDIATE; UPDATE deployment_permits SET holder=holder;")
        .unwrap();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        conn.raw().execute_batch("ROLLBACK").unwrap();
    })
}
#[test]
fn acquire_and_release_retry_shared_cache_locks_without_duplicate_permits() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    let clock = FixedClock(1_700_000_000_000_000);
    let gate = DeploymentGate(Ctx {
        conn: conn.raw(),
        clock: &clock,
    });
    let release = blocker(&db);
    let permit = gate.acquire_permit("scan", "worker").unwrap().unwrap();
    release.join().unwrap();
    assert_eq!(
        conn.raw()
            .query_row("SELECT count(*) FROM deployment_permits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let release = blocker(&db);
    assert!(gate.release_permit(&permit).unwrap());
    release.join().unwrap();
    assert!(!gate.release_permit(&permit).unwrap());
    assert_eq!(gate.status().unwrap().permits, 0);
    assert!(conn.raw().is_autocommit());
}

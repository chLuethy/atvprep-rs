//! Multiple workers and failed retry inserts cannot split durable recovery.
use atvprep_manager::{
    clock::FixedClock,
    db::{Conn, Db},
    repo::Ctx,
    schema,
    worker::recovery::Recovery,
};
use std::{
    sync::{Arc, Barrier},
    time::Duration,
};
fn seed(db: &Db) -> Conn {
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'test','/library');
    INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'test.mp4','video','.mp4',1,1,'test','present','unknown','idle','none');
    INSERT INTO jobs(id,action,status) VALUES(1,'verify','running');
    INSERT INTO job_items(id,job_id,media_file_id,action,position,status,attempt,executor_fencing_token,lease_expires_at) VALUES(1,1,1,'verify',1,'running',1,7,'2020-01-01');").unwrap();
    conn
}
#[test]
fn retry_insert_failure_rolls_back_interruption_and_parent_reconciliation() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let clock = FixedClock(1_700_000_000_000_000);
    conn.raw().execute_batch("CREATE TRIGGER refuse_retry BEFORE INSERT ON jobs WHEN NEW.trigger='retry' BEGIN SELECT RAISE(ABORT,'refused retry'); END;").unwrap();
    let recovery = Recovery(Ctx {
        conn: conn.raw(),
        clock: &clock,
    });
    assert!(recovery.recover_with_retries(100, 3).is_err());
    assert_eq!(
        conn.raw()
            .query_row("SELECT status FROM job_items WHERE id=1", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "running"
    );
    assert_eq!(
        conn.raw()
            .query_row("SELECT status FROM jobs WHERE id=1", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "running"
    );
    conn.raw()
        .execute_batch("DROP TRIGGER refuse_retry")
        .unwrap();
    assert_eq!(recovery.recover_with_retries(100, 3).unwrap().retried, 1);
    assert_eq!(recovery.recover_with_retries(100, 3).unwrap().retried, 0);
}
#[test]
fn concurrent_workers_create_exactly_one_retry_even_when_interruption_is_slow() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("recovery.sqlite")).unwrap();
    let conn = seed(&db);
    conn.raw().execute_batch("CREATE TRIGGER delay_recovery BEFORE UPDATE OF status ON job_items WHEN NEW.status='interrupted' BEGIN SELECT delay_recovery(); END;").unwrap();
    let start = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let db = db.clone();
        let start = start.clone();
        threads.push(std::thread::spawn(move || {
            let conn = db.conn().unwrap();
            conn.raw()
                .create_scalar_function(
                    "delay_recovery",
                    0,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                    |_| {
                        std::thread::sleep(Duration::from_millis(100));
                        Ok(1)
                    },
                )
                .unwrap();
            let clock = FixedClock(1_700_000_000_000_000);
            start.wait();
            Recovery(Ctx {
                conn: conn.raw(),
                clock: &clock,
            })
            .recover_with_retries(100, 3)
            .unwrap()
        }));
    }
    let summaries: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(
        summaries.iter().map(|result| result.retried).sum::<usize>(),
        1
    );
    assert_eq!(
        conn.raw()
            .query_row(
                "SELECT count(*) FROM jobs WHERE trigger='retry'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
#[test]
fn idle_parent_is_reconciled_even_without_an_expired_claim() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    conn.raw()
        .execute("UPDATE job_items SET status='failed' WHERE id=1", [])
        .unwrap();
    let clock = FixedClock(1_700_000_000_000_000);
    assert_eq!(
        Recovery(Ctx {
            conn: conn.raw(),
            clock: &clock
        })
        .recover_with_retries(100, 3)
        .unwrap()
        .reconciled_jobs,
        1
    );
    assert_eq!(
        conn.raw()
            .query_row("SELECT status FROM jobs WHERE id=1", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "failed"
    );
}

#[test]
fn paused_exhausted_and_changing_items_are_failed_without_automatic_retry() {
    for (action, attempt, paused, code) in [
        ("verify", 1, true, "gate_paused"),
        ("verify", 3, false, "max_attempts_exceeded"),
        ("fix", 1, false, "max_attempts_exceeded"),
        ("convert", 1, false, "max_attempts_exceeded"),
    ] {
        let db = Db::memory().unwrap();
        let conn = seed(&db);
        conn.raw()
            .execute(
                "UPDATE job_items SET action=?1,attempt=?2",
                rusqlite::params![action, attempt],
            )
            .unwrap();
        conn.raw()
            .execute("UPDATE jobs SET action=?1", [action])
            .unwrap();
        if paused {
            conn.raw()
                .execute("UPDATE deployment_gate SET mode='maintenance'", [])
                .unwrap();
        }
        let clock = FixedClock(1_700_000_000_000_000);
        let result = Recovery(Ctx {
            conn: conn.raw(),
            clock: &clock,
        })
        .recover_with_retries(100, 3)
        .unwrap();
        assert_eq!(result.failed, 1);
        assert_eq!(result.retried, 0);
        assert_eq!(
            conn.raw()
                .query_row("SELECT error_code FROM job_items WHERE id=1", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            code
        );
        assert_eq!(
            conn.raw()
                .query_row("SELECT status FROM job_items WHERE id=1", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            "failed"
        );
    }
}

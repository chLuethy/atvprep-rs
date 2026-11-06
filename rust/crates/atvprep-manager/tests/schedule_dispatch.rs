//! Slot dispatch, failure rollback, rotation and selection against actual SQLite.
use atvprep_manager::{
    clock::{Clock, FixedClock},
    db::{Conn, Db},
    repo::Ctx,
    schedules::{run_due_schedules, run_schedule},
    schema,
};
use chrono::{DateTime, Duration, Local, Utc};
use std::sync::{Arc, Barrier};
const NOW: i64 = 1_700_000_000_000_000;
fn seed(db: &Db) -> Conn {
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'test','/library'),(2,'other','/other');
    INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES
    (1,1,'one.mp4','video','.mp4',1,1,'one','present','clean','idle','none'),
    (2,1,'two.m4v','video','.m4v',1,1,'two','present','warning','idle','none'),
    (3,1,'three.mkv','video','.mkv',1,1,'three','present','warning','idle','none'),
    (4,2,'four.mov','video','.mov',1,1,'four','present','warning','idle','none'),
    (5,1,'missing.mp4','video','.mp4',1,1,'missing','missing','warning','idle','none');").unwrap();
    let time = (DateTime::<Utc>::from_timestamp_micros(NOW)
        .unwrap()
        .with_timezone(&Local)
        - Duration::minutes(30))
    .format("%H:%M")
    .to_string();
    conn.raw().execute("INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled,armed_at) VALUES(1,'test','verify',1,127,?1,1,1,'2000-01-01')",[time]).unwrap();
    conn
}
#[test]
fn a_failed_queue_insert_does_not_consume_the_slot() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    conn.raw().execute_batch("CREATE TRIGGER refuse_schedule BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT,'queue unavailable'); END;").unwrap();
    // The failure is contained to that schedule: the pass succeeds, nothing ran, the slot stays open.
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
    assert_eq!(
        conn.raw()
            .query_row("SELECT last_slot_at FROM schedules WHERE id=1", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .unwrap(),
        None
    );
    conn.raw()
        .execute_batch("DROP TRIGGER refuse_schedule")
        .unwrap();
    assert_eq!(run_due_schedules(ctx).unwrap(), 1);
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
}
#[test]
fn a_dispatched_slot_is_recorded_as_a_scheduled_run() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_due_schedules(ctx).unwrap(), 1);
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
    let (slot, last_slot, trigger, result, job): (String, String, String, String, Option<i64>) =
        conn.raw()
            .query_row(
                "SELECT r.slot, s.last_slot_at, r.trigger, r.result, r.job_id
                 FROM schedule_runs r JOIN schedules s ON s.id=r.schedule_id",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
    assert_eq!((slot, trigger.as_str()), (last_slot, "scheduled"));
    assert_eq!(result, "Queued 1 file");
    assert_eq!(job, Some(1));
    // A manual run adds an entry without a slot.
    run_schedule(ctx, 1).unwrap();
    let manual: i64 = conn
        .raw()
        .query_row(
            "SELECT count(*) FROM schedule_runs WHERE trigger='manual' AND slot IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(manual, 1);
}
#[test]
fn a_failing_or_corrupt_schedule_does_not_block_the_ones_after_it() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    // Schedule 1 (the seeded one) sorts first and is made to fail; 2 is corrupt; 3 is healthy.
    let time: String = conn
        .raw()
        .query_row("SELECT time_of_day FROM schedules WHERE id=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    conn.raw().execute("INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled,armed_at) VALUES(2,'bad','verify',1,127,'not-a-time',1,1,'2000-01-01'),(3,'good','verify',1,127,?1,1,1,'2000-01-01')",[time]).unwrap();
    conn.raw().execute_batch("CREATE TRIGGER refuse_first BEFORE UPDATE OF last_slot_at ON schedules WHEN NEW.id=1 BEGIN SELECT RAISE(ABORT,'first refused'); END;").unwrap();
    assert_eq!(run_due_schedules(ctx).unwrap(), 1);
}
fn local_minute_offset(minutes: i64) -> (String, String) {
    let at = DateTime::<Utc>::from_timestamp_micros(NOW)
        .unwrap()
        .with_timezone(&Local)
        + Duration::minutes(minutes);
    (at.format("%-M").to_string(), at.format("%-H").to_string())
}
#[test]
fn the_cron_decides_the_slot_not_the_legacy_columns() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    // The legacy columns say 00:00; the cron says half an hour ago, then half an hour ahead.
    conn.raw().execute("DELETE FROM schedules", []).unwrap();
    let (minute, hour) = local_minute_offset(-30);
    let cron = format!("{minute} {hour} * * *");
    conn.raw().execute("INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,max_files,enabled,armed_at,cron) VALUES(1,'c','verify',1,127,'00:00',1,1,'2000-01-01',?1)",[cron]).unwrap();
    assert_eq!(run_due_schedules(ctx).unwrap(), 1);
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
    let (minute, hour) = local_minute_offset(30);
    let cron = format!("{minute} {hour} * * *");
    conn.raw()
        .execute("UPDATE schedules SET cron=?1, last_slot_at=NULL", [cron])
        .unwrap();
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
}
#[test]
fn an_invalid_cron_is_skipped() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    conn.raw()
        .execute("UPDATE schedules SET cron='nonsense' WHERE id=1", [])
        .unwrap();
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
}
#[test]
fn competing_dispatchers_claim_each_slot_once() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(&temp.path().join("schedules.sqlite")).unwrap();
    let conn = seed(&db);
    let start = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let db = db.clone();
        let start = start.clone();
        workers.push(std::thread::spawn(move || {
            let conn = db.conn().unwrap();
            let clock = FixedClock(NOW);
            start.wait();
            run_due_schedules(Ctx {
                conn: conn.raw(),
                clock: &clock,
            })
            .unwrap()
        }));
    }
    assert_eq!(
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .sum::<usize>(),
        1
    );
    assert_eq!(
        conn.raw()
            .query_row("SELECT count(*) FROM jobs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn dispatch_respects_catchup_arming_pause_and_gate() {
    for mode in ["late", "newly-armed", "paused", "closed"] {
        let db = Db::memory().unwrap();
        let conn = seed(&db);
        let clock = FixedClock(NOW);
        match mode {
            "late" => {
                let time = (DateTime::<Utc>::from_timestamp_micros(NOW)
                    .unwrap()
                    .with_timezone(&Local)
                    - Duration::hours(7))
                .format("%H:%M")
                .to_string();
                conn.raw()
                    .execute("UPDATE schedules SET time_of_day=?1", [time])
                    .unwrap();
            }
            "newly-armed" => {
                conn.raw()
                    .execute("UPDATE schedules SET armed_at=?1", [clock.now()])
                    .unwrap();
            }
            "paused" => {
                conn.raw()
                    .execute("UPDATE schedules SET enabled=0", [])
                    .unwrap();
            }
            _ => {
                conn.raw()
                    .execute("UPDATE deployment_gate SET mode='maintenance'", [])
                    .unwrap();
            }
        }
        assert_eq!(
            run_due_schedules(Ctx {
                conn: conn.raw(),
                clock: &clock
            })
            .unwrap(),
            0,
            "{mode}"
        );
        assert_eq!(
            conn.raw()
                .query_row("SELECT last_slot_at FROM schedules", [], |row| row
                    .get::<_, Option<String>>(0))
                .unwrap(),
            None
        );
    }
}
#[test]
fn runs_rotate_across_present_mp4_files_and_respect_root_state_and_busy_files() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    let mut media = Vec::new();
    for _ in 0..3 {
        run_schedule(ctx, 1).unwrap();
        media.push(conn.raw().query_row("SELECT media_file_id FROM job_items WHERE job_id=(SELECT last_job_id FROM schedules WHERE id=1)",[],|row|row.get::<_,i64>(0)).unwrap());
    }
    assert_eq!(media, [1, 2, 1]);
    conn.raw()
        .execute("UPDATE schedules SET compliance_state='warning'", [])
        .unwrap();
    run_schedule(ctx, 1).unwrap();
    assert_eq!(conn.raw().query_row("SELECT media_file_id FROM job_items WHERE job_id=(SELECT last_job_id FROM schedules WHERE id=1)",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    conn.raw().execute_batch("INSERT INTO jobs(id,action,status) VALUES(100,'fix','queued'); INSERT INTO job_items(job_id,media_file_id,action,position,status) VALUES(100,2,'fix',1,'queued');").unwrap();
    assert_eq!(run_schedule(ctx, 1).unwrap(), "No matching files");
}
fn scan_schedule(conn: &Conn, root: Option<i64>) -> i64 {
    let time: String = conn
        .raw()
        .query_row("SELECT time_of_day FROM schedules WHERE id=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    conn.raw()
        .execute("UPDATE schedules SET enabled=0 WHERE id=1", [])
        .unwrap();
    conn.raw().execute("INSERT INTO schedules(id,name,action,root_id,days_mask,time_of_day,enabled,armed_at) VALUES(10,'scan','scan',?1,127,?2,1,'2000-01-01')",rusqlite::params![root,time]).unwrap();
    10
}
fn queued_scans(conn: &Conn) -> Vec<(i64, String)> {
    conn.raw()
        .prepare("SELECT root_id, trigger FROM scan_runs WHERE status='queued' ORDER BY root_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
#[test]
fn a_scan_schedule_queues_one_scan_of_its_library() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let id = scan_schedule(&conn, Some(2));
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_schedule(ctx, id).unwrap(), "Queued 1 scan");
    assert_eq!(queued_scans(&conn), vec![(2, "manual".to_owned())]);
    let (job, result): (Option<i64>, String) = conn
        .raw()
        .query_row(
            "SELECT last_job_id, last_result FROM schedules WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((job, result.as_str()), (None, "Queued 1 scan"));
    assert_eq!(
        conn.raw()
            .query_row("SELECT count(*) FROM jobs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn a_scan_schedule_without_a_library_scans_every_library() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    let id = scan_schedule(&conn, None);
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    assert_eq!(run_schedule(ctx, id).unwrap(), "Queued 2 scans");
    assert_eq!(queued_scans(&conn).len(), 2);
    // A second run while the scans still wait shares them.
    run_schedule(ctx, id).unwrap();
    assert_eq!(queued_scans(&conn).len(), 2);
}
#[test]
fn a_due_scan_schedule_queues_its_scan_once_and_the_closed_gate_blocks_it() {
    let db = Db::memory().unwrap();
    let conn = seed(&db);
    scan_schedule(&conn, Some(1));
    let clock = FixedClock(NOW);
    let ctx = Ctx {
        conn: conn.raw(),
        clock: &clock,
    };
    conn.raw()
        .execute("UPDATE deployment_gate SET mode='maintenance'", [])
        .unwrap();
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
    assert!(queued_scans(&conn).is_empty());
    conn.raw()
        .execute("UPDATE deployment_gate SET mode='open'", [])
        .unwrap();
    assert_eq!(run_due_schedules(ctx).unwrap(), 1);
    assert_eq!(queued_scans(&conn), vec![(1, "manual".to_owned())]);
    assert_eq!(run_due_schedules(ctx).unwrap(), 0);
}

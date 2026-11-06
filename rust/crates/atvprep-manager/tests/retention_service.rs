//! Full maintenance passes are bounded, preserve retained data, and roll back on lease loss.
use atvprep_manager::{
    clock::{Clock, FixedClock},
    db::{Conn, Db},
    repo::Ctx,
    schema,
    worker::service::{WorkerLimits, WorkerService},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
const NOW: i64 = 1_700_000_000_000_000;
fn count(conn: &Conn, table: &str) -> i64 {
    conn.raw()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}
fn seed(conn: &Conn) {
    conn.raw().execute_batch("INSERT INTO auth_sessions(id,session_token_hash,csrf_secret_hash,expires_at) VALUES(1,'old','csrf-old','2020-01-01'),(2,'recent','csrf-recent','2023-11-10'),(3,'active','csrf-active','2999-01-01');
    INSERT INTO csrf_tokens(auth_session_id,token_hash,method,normalized_path,action,expires_at) VALUES(3,'old-proof','POST','/test','test','2020-01-01'),(3,'recent-proof','POST','/test','test','2023-11-10');
    INSERT INTO idempotency_records(method,normalized_route,key,request_hash,response_status,response_json,expires_at) VALUES('POST','/test','old','hash',200,'{}','2020-01-01'),('POST','/test','recent','hash',200,'{}','2023-11-10');
    INSERT INTO deployment_permits(id,kind,holder,token,generation,released_at) VALUES(1,'job','old','one',0,'2020-01-01'),(2,'job','open','two',0,NULL),(3,'job','recent','three',0,'2023-11-10');
    INSERT INTO jobs(id,action,status,finished_at) VALUES(1,'verify','failed','2020-01-01'),(2,'verify','failed','2020-01-01');
    INSERT INTO events(job_id,topic,entity_type,entity_id,payload_json,expires_at) VALUES(1,'kept','job',1,'{}','2999-01-01'),(NULL,'old','job',1,'{}','2020-01-01'),(NULL,'recent','job',1,'{}','2023-11-10');
    INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'test','/library');
    INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'test.mp4','video','.mp4',1,1,'test','present','unknown','idle','none');
    INSERT INTO job_items(job_id,media_file_id,action,position,status,log_tail) VALUES(1,1,'verify',1,'failed','old log'),(2,1,'verify',1,'failed','old log');
    INSERT INTO inspections(media_file_id,source_fingerprint,input_fingerprint,tool_name,tool_version,result,created_at) VALUES(1,'a','a','ffprobe','1','{}','2020-01-01'),(1,'b','b','ffprobe','1','{}','2020-01-02');").unwrap();
}
#[test]
fn all_categories_obey_retention_grace_and_keep_the_latest_inspection() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    seed(&conn);
    let clock = FixedClock(NOW);
    let service = WorkerService::new(
        Ctx {
            conn: conn.raw(),
            clock: &clock,
        },
        &conn,
    );
    let result = service
        .tick(
            "maintenance",
            WorkerLimits {
                maintenance_batch: 1,
                ..WorkerLimits::default()
            },
        )
        .unwrap();
    assert_eq!(result.events_pruned, 1);
    for (table, expected) in [
        ("auth_sessions", 2),
        ("csrf_tokens", 1),
        ("idempotency_records", 1),
        ("deployment_permits", 2),
        ("deployment_events", 1),
        ("events", 2),
        ("jobs", 1),
        ("inspections", 1),
    ] {
        assert_eq!(count(&conn, table), expected, "{table}");
    }
    assert_eq!(
        conn.raw()
            .query_row("SELECT log_tail FROM job_items WHERE job_id=1", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .unwrap(),
        None
    );
    assert_eq!(
        conn.raw()
            .query_row("SELECT source_fingerprint FROM inspections", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "b"
    );
    assert_eq!(
        conn.raw()
            .query_row("SELECT owner_id FROM maintenance_leases", [], |row| row
                .get::<_, Option<
                String,
            >>(
                0
            ))
            .unwrap(),
        None
    );
}
#[test]
fn maintenance_keeps_the_last_ten_runs_per_schedule() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    conn.raw().execute_batch("INSERT INTO schedules(id,name,action,days_mask,time_of_day) VALUES(1,'a','scan',127,'03:00'),(2,'b','scan',127,'03:00');").unwrap();
    for id in 1..=12 {
        conn.raw().execute("INSERT INTO schedule_runs(schedule_id,started_at,trigger,result) VALUES(1,'2023-01-01','manual',?1)", [format!("run {id}")]).unwrap();
    }
    conn.raw().execute("INSERT INTO schedule_runs(schedule_id,started_at,trigger,result) VALUES(2,'2023-01-01','manual','only')", []).unwrap();
    let clock = FixedClock(NOW);
    let service = WorkerService::new(
        Ctx {
            conn: conn.raw(),
            clock: &clock,
        },
        &conn,
    );
    let result = service
        .tick("maintenance", WorkerLimits::default())
        .unwrap();
    assert!(result.other_records_pruned >= 2);
    let kept: Vec<String> = conn
        .raw()
        .prepare("SELECT result FROM schedule_runs WHERE schedule_id=1 ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(kept.len(), 10);
    assert_eq!(kept[0], "run 3");
    assert_eq!(count(&conn, "schedule_runs"), 11);
}
struct Expiring(Arc<AtomicBool>);
impl Clock for Expiring {
    fn now_micros(&self) -> i64 {
        NOW + if self.0.load(Ordering::Acquire) {
            60_000_000
        } else {
            0
        }
    }
}
#[test]
fn expiry_during_a_destructive_pass_rolls_back_every_delete() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    seed(&conn);
    let expired = Arc::new(AtomicBool::new(false));
    let signal = expired.clone();
    conn.raw()
        .create_scalar_function(
            "expire_maintenance",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |_| {
                signal.store(true, Ordering::Release);
                Ok(1)
            },
        )
        .unwrap();
    conn.raw().execute_batch("CREATE TRIGGER expire_after_delete AFTER DELETE ON auth_sessions BEGIN SELECT expire_maintenance(); END;").unwrap();
    let clock = Expiring(expired.clone());
    assert!(
        WorkerService::new(
            Ctx {
                conn: conn.raw(),
                clock: &clock
            },
            &conn
        )
        .tick("maintenance", WorkerLimits::default())
        .is_err()
    );
    assert!(expired.load(Ordering::Acquire));
    assert_eq!(count(&conn, "auth_sessions"), 3);
    assert_eq!(count(&conn, "idempotency_records"), 2);
    assert_eq!(count(&conn, "events"), 3);
    assert_eq!(count(&conn, "maintenance_leases"), 0);
}

#[test]
fn runtime_retention_overrides_apply_and_invalid_policy_changes_nothing() {
    let db = Db::memory().unwrap();
    let mut conn = db.conn().unwrap();
    schema::migrate(&mut conn).unwrap();
    seed(&conn);
    conn.raw().execute_batch("INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('job_retention_days','365','int',1),('log_retention_days','365','int',1);").unwrap();
    conn.raw()
        .execute("UPDATE jobs SET finished_at='2023-10-01'", [])
        .unwrap();
    let clock = FixedClock(NOW);
    let service = WorkerService::new(
        Ctx {
            conn: conn.raw(),
            clock: &clock,
        },
        &conn,
    );
    service
        .tick("maintenance", WorkerLimits::default())
        .unwrap();
    assert_eq!(count(&conn, "jobs"), 2);
    assert_eq!(
        conn.raw()
            .query_row(
                "SELECT count(*) FROM job_items WHERE log_tail IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    conn.raw()
        .execute(
            "UPDATE runtime_settings SET value_json='-1' WHERE key='job_retention_days'",
            [],
        )
        .unwrap();
    let before = count(&conn, "events");
    assert!(
        service
            .tick("maintenance", WorkerLimits::default())
            .is_err()
    );
    assert_eq!(count(&conn, "events"), before);
}

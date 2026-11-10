//! Long-lived scanner, executor and maintenance loops with coordinated shutdown.
use super::{
    execution::{ExecutionConfig, execute_claimed},
    scan_service::{self, ScanOptions},
    service::{WorkerLimits, WorkerService},
};
use crate::{
    Error, Result, clock::Clock, db::Db, deployment_gate::DeploymentGate, jobs::Jobs,
    leases::MediaExecutorLease, repo::Ctx, scheduler::Scheduler,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone)]
pub struct RuntimeConfig {
    pub owner: String,
    pub execution: ExecutionConfig,
    pub ffprobe_executable: PathBuf,
    pub scan: ScanOptions,
    pub scan_interval_seconds: i64,
    pub maintenance: WorkerLimits,
}

/// Owned worker threads. Dropping the handle requests shutdown and joins the loops.
pub struct WorkerHandle {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    health: Vec<Arc<AtomicBool>>,
}

impl WorkerHandle {
    #[must_use]
    pub fn healthy(&self) -> bool {
        self.health.iter().all(|flag| flag.load(Ordering::Acquire))
            && !self.stop.load(Ordering::Acquire)
            && self.threads.iter().all(|thread| !thread.is_finished())
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.request_stop();
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

fn pause(stop: &AtomicBool) {
    for _ in 0..10 {
        if stop.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Start independent scanner, execution and maintenance loops.
///
/// # Errors
/// Rejects invalid limits or a failed worker-thread startup.
pub fn start(db: &Db, clock: &Arc<dyn Clock>, config: &RuntimeConfig) -> Result<WorkerHandle> {
    if config.owner.is_empty() || config.scan_interval_seconds < 1 || config.scan.lease_seconds < 3
    {
        return Err(Error::Invalid(
            "invalid worker runtime configuration".to_owned(),
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut handle = WorkerHandle {
        stop: stop.clone(),
        threads: vec![],
        health: vec![],
    };
    for role in 0..3 {
        let db = db.clone();
        let clock = clock.clone();
        let mut config = config.clone();
        config.execution.shutdown = Some(stop.clone());
        config.scan.shutdown = Some(stop.clone());
        let stop = stop.clone();
        let health = Arc::new(AtomicBool::new(false));
        handle.health.push(health.clone());
        let thread = thread::Builder::new()
            .name(format!("atvprep-worker-{role}"))
            .spawn(move || {
                let mut swept = false;
                while !stop.load(Ordering::Acquire) {
                    let result = (|| {
                        let conn = db.conn()?;
                        let ctx = Ctx {
                            conn: conn.raw(),
                            clock: clock.as_ref(),
                        };
                        match role {
                            0 => scan_pass(ctx, &config),
                            1 => execute_pass(ctx, &config),
                            _ => {
                                WorkerService::new(ctx, &conn)
                                    .tick(&config.owner, config.maintenance)?;
                                // Once per start, after recovery; retried while the gate is shut.
                                if !swept {
                                    swept = super::orphans::sweep(ctx)?.ran;
                                }
                                Ok(())
                            }
                        }
                    })();
                    health.store(result.is_ok(), Ordering::Release);
                    pause(&stop);
                }
                health.store(false, Ordering::Release);
            })
            .map_err(|_| Error::Invalid("cannot start worker thread".to_owned()))?;
        handle.threads.push(thread);
    }
    Ok(handle)
}

fn live_scan_policy(ctx: Ctx<'_>, interval: i64, scan: &ScanOptions) -> Result<(i64, ScanOptions)> {
    let integer = |key: &str, fallback: i64, min: i64| -> Result<i64> {
        crate::settings::Settings(ctx)
            .get(key)?
            .map_or(Ok(fallback), |setting| {
                setting
                    .value
                    .as_i64()
                    .filter(|value| (min..=86400).contains(value))
                    .ok_or_else(|| Error::Invalid("invalid live scan setting".to_owned()))
            })
    };
    let mut scan = scan.clone();
    let interval = integer("scan_interval_seconds", interval, 5)?;
    scan.settle_seconds = integer("settle_seconds", scan.settle_seconds, 0)?;
    if let Some(setting) = crate::settings::Settings(ctx).get("scan_exclude_dirs")? {
        scan.excluded = crate::settings::normalize_excluded_dirs(&setting.value)?;
    }
    Ok((interval, scan))
}

fn scan_pass(ctx: Ctx<'_>, config: &RuntimeConfig) -> Result<()> {
    let gate = DeploymentGate(ctx).status()?;
    if !gate.healthy || gate.mode != "open" || gate.phase != "steady" {
        return Ok(());
    }
    let (interval, options) = live_scan_policy(ctx, config.scan_interval_seconds, &config.scan)?;
    let mut scheduler = Scheduler::new(ctx, &config.owner, options.lease_seconds);
    if !scheduler.acquire(interval)? {
        return Ok(());
    }
    let result = (|| {
        if let Some(scan) = scheduler.claim_manual_scan()? {
            scan_service::run(
                ctx,
                scan.id,
                &config.owner,
                scheduler.lease.fencing_token.unwrap_or(0),
                &options,
            )?;
        }
        let before = crate::clock::format_micros(
            ctx.clock
                .now_micros()
                .saturating_sub(interval.saturating_mul(1_000_000)),
        );
        let mut statement=ctx.conn.prepare("SELECT r.id,COALESCE((SELECT max(generation) FROM scan_runs WHERE root_id=r.id),0)+1 FROM library_roots r WHERE r.enabled=1 AND NOT EXISTS(SELECT 1 FROM scan_runs s WHERE s.root_id=r.id AND (s.status IN ('queued','running') OR s.started_at>?1)) ORDER BY r.id")?;
        let roots = statement
            .query_map([before], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (root, generation) in roots {
            if scheduler.create_scheduled_scan(root, generation)? {
                let id: i64 = ctx.conn.query_row(
                    "SELECT id FROM scan_runs WHERE root_id=?1 AND generation=?2",
                    rusqlite::params![root, generation],
                    |row| row.get(0),
                )?;
                scan_service::run(
                    ctx,
                    id,
                    &config.owner,
                    scheduler.lease.fencing_token.unwrap_or(0),
                    &options,
                )?;
            }
        }
        Ok(())
    })();
    scheduler.shutdown(3);
    result
}

fn execute_pass(ctx: Ctx<'_>, config: &RuntimeConfig) -> Result<()> {
    let gate = DeploymentGate(ctx).status()?;
    if !gate.healthy || gate.mode != "open" || gate.phase != "steady" {
        return Ok(());
    }
    let (_, scan) = live_scan_policy(ctx, config.scan_interval_seconds, &config.scan)?;
    queue_probes(ctx, scan.settle_seconds)?;
    queue_need_checks(ctx, scan.settle_seconds)?;
    let mut lease = MediaExecutorLease::new(ctx);
    if !lease.acquire(&config.owner, config.execution.lease_seconds)? {
        return Ok(());
    }
    let result = (|| {
        let mut statement=ctx.conn.prepare("SELECT DISTINCT j.id FROM jobs j JOIN job_items i ON i.job_id=j.id WHERE j.status IN ('queued','running') AND i.status='queued' AND j.action IN ('inspect','verify','fix','convert') ORDER BY j.id LIMIT 1")?;
        let jobs = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for job in jobs {
            let fence = lease.fencing_token.unwrap_or(0);
            if let Some(item) = Jobs(ctx).claim_media_item(
                job,
                &config.owner,
                fence,
                config.execution.lease_seconds,
                1,
            )? {
                let mut execution = config.execution.clone();
                if item.action == "inspect" {
                    execution.executable.clone_from(&config.ffprobe_executable);
                }
                execute_claimed(ctx, item.id, &config.owner, fence, &execution)?;
            }
        }
        Ok(())
    })();
    let release = lease.release();
    match result {
        Ok(()) => {
            release?;
            Ok(())
        }
        Err(error) => {
            let _ = release;
            Err(error)
        }
    }
}

/// How many background checks of an unknown Kodierbedarf may wait at a time.
const NEED_CHECK_BATCH: i64 = 32;

/// Queue „Prüfen“ for files whose Kodierbedarf is unknown. Each file and state of its bytes is tried
/// once: a check that ends without a need is not repeated on its own, a changed file is checked anew.
fn queue_need_checks(ctx: Ctx<'_>, settle_seconds: i64) -> Result<()> {
    let tx =
        rusqlite::Transaction::new_unchecked(ctx.conn, rusqlite::TransactionBehavior::Immediate)?;
    let write = Ctx {
        conn: &tx,
        clock: ctx.clock,
    };
    let gate = DeploymentGate(write).status()?;
    if gate.healthy && gate.mode == "open" && gate.phase == "steady" {
        let active: i64 = tx.query_row(
            "SELECT count(*) FROM job_items WHERE action='verify' AND status IN ('queued','claimed','running')
               AND json_extract(settings_snapshot_json,'$.need_check')=1",
            [],
            |row| row.get(0),
        )?;
        let available = NEED_CHECK_BATCH.saturating_sub(active).max(0);
        let cutoff = ctx
            .clock
            .now_micros()
            .saturating_mul(1000)
            .saturating_sub(settle_seconds.max(0).saturating_mul(1_000_000_000));
        let mut statement = tx.prepare(&format!(
            "SELECT m.id FROM media_files m
             WHERE m.discovery_state='present' AND m.encode_need='unknown' AND m.mtime_ns<=?1
               AND NOT EXISTS(SELECT 1 FROM job_items i WHERE i.media_file_id=m.id
                              AND i.status NOT IN {})
               AND NOT EXISTS(SELECT 1 FROM job_items i JOIN input_snapshots s ON s.job_item_id=i.id
                              WHERE i.media_file_id=m.id AND i.action='verify'
                                AND i.status NOT IN ('cancelled','stale_input')
                                AND json_extract(i.settings_snapshot_json,'$.need_check')=1
                                AND json_extract(s.snapshot_json,'$.file_fingerprint')=m.fingerprint)
             ORDER BY m.id LIMIT ?2",
            crate::state_machine::ItemStatus::terminal_sql_list()
        ))?;
        let ids = statement
            .query_map(rusqlite::params![cutoff, available], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        if !ids.is_empty() {
            let mut job = crate::jobs::NewJob::new("verify", &ids);
            job.settings_snapshot = serde_json::json!({"need_check": true});
            Jobs(write).create(&job)?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn queue_probes(ctx: Ctx<'_>, settle_seconds: i64) -> Result<()> {
    let tx =
        rusqlite::Transaction::new_unchecked(ctx.conn, rusqlite::TransactionBehavior::Immediate)?;
    let write = Ctx {
        conn: &tx,
        clock: ctx.clock,
    };
    let gate = DeploymentGate(write).status()?;
    if gate.healthy && gate.mode == "open" && gate.phase == "steady" {
        let active:i64=tx.query_row("SELECT count(*) FROM job_items WHERE action='inspect' AND status IN ('queued','claimed','running')",[],|row|row.get(0))?;
        let available = 32_i64.saturating_sub(active).max(0);
        let cutoff = ctx
            .clock
            .now_micros()
            .saturating_mul(1000)
            .saturating_sub(settle_seconds.max(0).saturating_mul(1_000_000_000));
        let mut statement=tx.prepare("SELECT m.id FROM media_files m WHERE m.discovery_state='present' AND m.mtime_ns<=?1 AND NOT EXISTS(SELECT 1 FROM inspections n WHERE n.media_file_id=m.id AND n.tool_name='ffprobe' AND n.input_fingerprint=m.fingerprint) AND NOT EXISTS(SELECT 1 FROM job_items i JOIN input_snapshots s ON s.job_item_id=i.id WHERE i.media_file_id=m.id AND i.action='inspect' AND i.status NOT IN ('cancelled','stale_input') AND json_extract(s.snapshot_json,'$.file_fingerprint')=m.fingerprint) ORDER BY m.id LIMIT ?2")?;
        let ids = statement
            .query_map(rusqlite::params![cutoff, available], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        if !ids.is_empty() {
            Jobs(write).create(&crate::jobs::NewJob::new("inspect", &ids))?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{clock::FixedClock, schema};

    /// A database with four present files (ids 1-4), of bytes that have long settled.
    fn library() -> (crate::db::Conn, FixedClock) {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw()
            .execute_batch(
                "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'r','/r');
                 INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,
                   discovery_state,compliance_state,workflow_state,desired_action,encode_need)
                 VALUES (1,1,'unknown.mp4','movie','.mp4',1,1,'a','present','unknown','idle','none','unknown'),
                        (2,1,'known.mp4','movie','.mp4',1,1,'b','present','unknown','idle','none','audio'),
                        (3,1,'gone.mp4','movie','.mp4',1,1,'c','missing','unknown','idle','none','unknown'),
                        (4,1,'other.mkv','movie','.mkv',1,1,'d','present','unknown','idle','none','unknown');",
            )
            .unwrap();
        (conn, FixedClock(1_700_000_000_000_000))
    }

    /// The media of every background check queued so far, in order.
    fn checked(conn: &crate::db::Conn) -> Vec<i64> {
        let mut statement = conn
            .raw()
            .prepare("SELECT media_file_id FROM job_items WHERE action='verify' ORDER BY id")
            .unwrap();
        let rows = statement.query_map([], |row| row.get(0)).unwrap();
        rows.collect::<std::result::Result<_, _>>().unwrap()
    }

    #[test]
    fn an_unknown_need_is_queued_for_a_check_and_a_known_one_is_not() {
        let (conn, clock) = library();
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        queue_need_checks(ctx, 0).unwrap();
        assert_eq!(checked(&conn), [1, 4]);
    }

    #[test]
    fn a_check_is_queued_once_and_not_repeated_after_it_ended_without_a_need() {
        let (conn, clock) = library();
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        queue_need_checks(ctx, 0).unwrap();
        queue_need_checks(ctx, 0).unwrap();
        assert_eq!(checked(&conn), [1, 4], "nothing is queued twice");
        conn.raw()
            .execute("UPDATE job_items SET status='failed'", [])
            .unwrap();
        queue_need_checks(ctx, 0).unwrap();
        assert_eq!(checked(&conn), [1, 4], "a failed check is not retried");
    }

    #[test]
    fn a_changed_file_is_checked_again() {
        let (conn, clock) = library();
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        queue_need_checks(ctx, 0).unwrap();
        conn.raw()
            .execute("UPDATE job_items SET status='succeeded'", [])
            .unwrap();
        conn.raw()
            .execute("UPDATE media_files SET fingerprint='a2' WHERE id=1", [])
            .unwrap();
        queue_need_checks(ctx, 0).unwrap();
        assert_eq!(checked(&conn), [1, 4, 1]);
    }

    #[test]
    fn a_file_with_a_job_running_is_left_alone() {
        let (conn, clock) = library();
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        Jobs(ctx)
            .create(&crate::jobs::NewJob::new("inspect", &[1]))
            .unwrap();
        queue_need_checks(ctx, 0).unwrap();
        assert_eq!(checked(&conn), [4]);
    }

    #[test]
    fn a_file_still_settling_waits_for_a_later_tick() {
        let (conn, clock) = library();
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        conn.raw()
            .execute(
                "UPDATE media_files SET mtime_ns=?1",
                [1_700_000_000_000_000_000_i64],
            )
            .unwrap();
        queue_need_checks(ctx, 120).unwrap();
        assert!(checked(&conn).is_empty());
    }
    #[test]
    fn scan_policy_reads_runtime_overrides_again_without_restarting_workers() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let base = ScanOptions {
            follow_mounts: false,
            excluded: vec![],
            settle_seconds: 120,
            lease_seconds: 30,
            shutdown: None,
        };
        assert_eq!(live_scan_policy(ctx, 3600, &base).unwrap().0, 3600);
        conn.raw().execute_batch("INSERT INTO runtime_settings(key,value_json,value_type,revision) VALUES('scan_interval_seconds','10','int',1),('settle_seconds','0','int',1),('scan_exclude_dirs','[\"skip\"]','list',1);").unwrap();
        let (interval, scan) = live_scan_policy(ctx, 3600, &base).unwrap();
        assert_eq!(interval, 10);
        assert_eq!(scan.settle_seconds, 0);
        assert_eq!(scan.excluded, ["skip"]);
        assert_eq!(base.settle_seconds, 120);
        conn.raw()
            .execute(
                "UPDATE runtime_settings SET value_json='5' WHERE key='scan_interval_seconds'",
                [],
            )
            .unwrap();
        assert_eq!(live_scan_policy(ctx, 3600, &base).unwrap().0, 5);
        conn.raw()
            .execute(
                "UPDATE runtime_settings SET value_json='null' WHERE key='scan_interval_seconds'",
                [],
            )
            .unwrap();
        assert!(live_scan_policy(ctx, 3600, &base).is_err());
    }
}

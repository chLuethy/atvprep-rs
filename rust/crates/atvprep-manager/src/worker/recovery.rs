//! Fail-closed recovery for items left claimed or running by a stopped worker.

use std::collections::BTreeSet;

use rusqlite::OptionalExtension;

use crate::deployment_gate::DeploymentGate;
use crate::jobs::{Jobs, NewJob};
use crate::models::JobItem;
use crate::repo::Ctx;
use crate::state_machine::{ItemStatus, JobStateMachine};
use crate::{Error, Result};

const DEFAULT_LIMIT: i64 = 100;
const WORKER_STOPPED: &str = "The worker stopped or lost its lease while this ran.";

/// The result of one bounded recovery pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoverySummary {
    /// Items atomically changed from `claimed` or `running` to `interrupted`.
    pub recovered: usize,
    /// Parents successfully reconciled after their active work disappeared.
    pub reconciled_jobs: usize,
    /// New non-destructive retry jobs created while the gate was open.
    pub retried: usize,
    /// Expired items explicitly failed because retry is forbidden, paused or exhausted.
    pub failed: usize,
}

/// Recovery operations performed against a single manager connection.
pub struct Recovery<'a>(pub Ctx<'a>);

impl Recovery<'_> {
    /// Mark at most `limit` expired claimed/running items as interrupted and reconcile idle parents.
    ///
    /// The update predicates include the recorded fencing token and the expired lease. Concurrent
    /// recovery passes therefore have exactly one winner for an item, and a worker which renewed
    /// its lease before the write is left untouched.
    ///
    /// # Errors
    ///
    /// `limit` is not positive, SQLite fails, or a parent cannot be reconciled.
    pub fn recover_expired_items(&self, limit: i64) -> Result<RecoverySummary> {
        if limit < 1 {
            return Err(Error::Invalid("recovery limit must be positive".to_owned()));
        }
        let now = self.0.clock.now();
        let expired = self.expired_items(limit)?;

        let mut affected = BTreeSet::new();
        let mut summary = RecoverySummary::default();
        for item in expired {
            let changed = self.0.conn.execute(
                "UPDATE job_items SET status='interrupted', status_version=status_version+1,
                    finished_at=COALESCE(finished_at, ?1), error_code='worker_stopped',
                    error_detail=?2
                 WHERE id=?3 AND status IN ('claimed', 'running')
                   AND lease_expires_at IS NOT NULL AND lease_expires_at <= ?1
                   AND (executor_fencing_token IS NULL OR executor_fencing_token=?4)",
                rusqlite::params![now, WORKER_STOPPED, item.id, item.executor_fencing_token],
            )?;
            if changed == 1 {
                summary.recovered += 1;
                affected.insert(item.job_id);
            }
        }
        summary.reconciled_jobs = self.reconcile_jobs(&affected)?;
        Ok(summary)
    }

    /// Recover expired work and queue bounded retries for `inspect` and `verify` only.
    ///
    /// A retry is new work, so the gate is checked after each successful interruption. `fix` and
    /// `convert` are never retried here: repeating a media mutation requires a fresh manual action.
    ///
    /// # Errors
    ///
    /// See [`Self::recover_expired_items`], or an eligible retry cannot be queued.
    pub fn recover_with_retries(&self, limit: i64, max_attempts: i64) -> Result<RecoverySummary> {
        if max_attempts < 1 {
            return Err(Error::Invalid("max_attempts must be positive".to_owned()));
        }
        let transaction = if self.0.conn.is_autocommit() {
            Some(rusqlite::Transaction::new_unchecked(
                self.0.conn,
                rusqlite::TransactionBehavior::Immediate,
            )?)
        } else {
            None
        };
        let before = self.expired_items(limit)?;
        // Read before recovery: settling the items also settles a cancelled parent.
        let mut cancelled_jobs = BTreeSet::new();
        for item in &before {
            let cancelled: bool = self.0.conn.query_row(
                "SELECT status IN ('cancel_requested','cancelled') FROM jobs WHERE id=?1",
                [item.job_id],
                |row| row.get(0),
            )?;
            if cancelled {
                cancelled_jobs.insert(item.job_id);
            }
        }
        let mut summary = self.recover_expired_items(limit)?;
        for item in before {
            let current: Option<String> = self
                .0
                .conn
                .query_row(
                    "SELECT status FROM job_items WHERE id=?1",
                    [item.id],
                    |row| row.get(0),
                )
                .optional()?;
            if current.as_deref() != Some("interrupted") {
                continue;
            }
            // Work the user cancelled must not come back as a retry.
            if cancelled_jobs.contains(&item.job_id) {
                continue;
            }
            let status = DeploymentGate(self.0).status()?;
            let changes_media = matches!(item.action.as_str(), "fix" | "convert");
            let paused = (!status.healthy || status.mode != "open" || status.phase != "steady")
                && item.attempt < max_attempts
                && !changes_media;
            if changes_media || item.attempt >= max_attempts || paused {
                let code = if paused {
                    "gate_paused"
                } else {
                    "max_attempts_exceeded"
                };
                let detail = if paused {
                    "The worker stopped while this ran, and new work is paused for a release switch. Start it again by hand once the switch is done.".to_owned()
                } else {
                    format!(
                        "The worker stopped while this ran, {max_attempts} times in a row. The manager gave up; start it again by hand."
                    )
                };
                summary.failed+=self.0.conn.execute("UPDATE job_items SET status='failed',status_version=status_version+1,error_code=?1,error_detail=?2 WHERE id=?3 AND status='interrupted'",rusqlite::params![code,detail,item.id])?;
                continue;
            }
            self.0.conn.execute("UPDATE job_items SET error_detail=?1 WHERE id=?2 AND status='interrupted'",rusqlite::params!["The worker stopped or lost its lease while this ran, so the work was lost. It was queued again as a new job.",item.id])?;
            let media_ids = [item.media_file_id];
            let mut retry = NewJob::new(&item.action, &media_ids);
            retry.trigger = "retry";
            retry.settings_snapshot = item.settings_snapshot.clone();
            retry
                .input_snapshots
                .insert(item.media_file_id, item.input_snapshot.clone());
            let job = Jobs(self.0).create(&retry)?;
            self.0.conn.execute(
                "UPDATE job_items SET attempt=?1, origin_item_id=?2 WHERE job_id=?3",
                rusqlite::params![item.attempt, item.id, job.id],
            )?;
            summary.retried += 1;
        }
        self.interrupt_stranded_scans()?;
        summary.reconciled_jobs += self.reconcile_idle_jobs()?;
        if let Some(transaction) = transaction {
            transaction.commit()?;
        }
        Ok(summary)
    }

    fn expired_items(&self, limit: i64) -> Result<Vec<JobItem>> {
        if limit < 1 {
            return Err(Error::Invalid("recovery limit must be positive".to_owned()));
        }
        let now = self.0.clock.now();
        let mut statement = self.0.conn.prepare(
            "SELECT * FROM job_items
             WHERE status IN ('claimed', 'running')
               AND lease_expires_at IS NOT NULL AND lease_expires_at <= ?1
             ORDER BY lease_expires_at, id LIMIT ?2",
        )?;
        let items = statement
            .query_map(
                rusqlite::params![now, limit.min(DEFAULT_LIMIT)],
                JobItem::from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(items)
    }

    /// Reconcile open jobs that have no queued or running item left.
    ///
    /// # Errors
    ///
    /// SQLite fails or the durable job state machine refuses a concurrent/illegal update.
    pub fn reconcile_idle_jobs(&self) -> Result<usize> {
        let cancelled = self.finalize_cancel_requested_jobs()?;
        let mut statement = self.0.conn.prepare(
            "SELECT id FROM jobs
             WHERE status IN ('queued', 'running')
               AND EXISTS (SELECT 1 FROM job_items WHERE job_id=jobs.id)
               AND NOT EXISTS (
                 SELECT 1 FROM job_items WHERE job_id=jobs.id
                   AND status IN ('queued','claimed','running','cancel_requested','cancelling')
               ) ORDER BY id LIMIT 100",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<std::result::Result<BTreeSet<_>, _>>()?;
        drop(statement);
        Ok(cancelled + self.reconcile_jobs(&ids)?)
    }

    /// Mark scan runs whose scheduler lease is gone as `interrupted`.
    ///
    /// A claimed or running scan owned by a lease holder is stranded when that lease expired or was
    /// taken over (a crash). Left alone it counts as active work for the deployment gate and keeps
    /// its root from ever being scheduled again. Read first: most passes find nothing and must not
    /// take the write lock away from a running scan.
    fn interrupt_stranded_scans(&self) -> Result<usize> {
        let now = self.0.clock.now();
        let stranded = "status IN ('claimed','running') AND claimed_by IS NOT NULL
             AND NOT EXISTS (SELECT 1 FROM scheduler_leases
               WHERE id=1 AND owner_id=scan_runs.claimed_by
                 AND fencing_token=scan_runs.fencing_token AND lease_expires_at > ?1)";
        let any: bool = self.0.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM scan_runs WHERE {stranded})"),
            [&now],
            |row| row.get(0),
        )?;
        if !any {
            return Ok(0);
        }
        Ok(self.0.conn.execute(
            &format!("UPDATE scan_runs SET status='interrupted', finished_at=?1 WHERE {stranded}"),
            [&now],
        )?)
    }

    /// Cancel the queued items of jobs a user cancelled, and close each job once nothing is active.
    ///
    /// Nothing else ever moves a queued item of a `cancel_requested` job, so without this the job
    /// stays open, blocks its files and cannot be cleared or retried.
    fn finalize_cancel_requested_jobs(&self) -> Result<usize> {
        let mut statement = self
            .0
            .conn
            .prepare("SELECT id FROM jobs WHERE status='cancel_requested' ORDER BY id LIMIT 100")?;
        let jobs = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let states = JobStateMachine(self.0);
        let mut closed = 0;
        for job in jobs {
            let mut queued = self.0.conn.prepare(
                "SELECT id, status_version FROM job_items WHERE job_id=?1 AND status='queued'",
            )?;
            let items = queued
                .query_map([job], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(queued);
            for (item, version) in items {
                states.transition_item(item, ItemStatus::Cancelled, version, None, None)?;
            }
            let (active, version): (i64, i64) = self.0.conn.query_row(
                "SELECT (SELECT count(*) FROM job_items WHERE job_id=jobs.id
                    AND status IN ('queued','claimed','running','cancel_requested','cancelling')),
                 status_version FROM jobs WHERE id=?1",
                [job],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if active == 0 && states.aggregate_job(job, version)?.changed {
                closed += 1;
            }
        }
        Ok(closed)
    }

    fn reconcile_jobs(&self, ids: &BTreeSet<i64>) -> Result<usize> {
        let states = JobStateMachine(self.0);
        let mut reconciled = 0;
        for id in ids {
            let before = self.0.conn.query_row(
                "SELECT status_version FROM jobs WHERE id=?1",
                [id],
                |row| row.get(0),
            )?;
            if states.aggregate_job(*id, before)?.changed {
                reconciled += 1;
            }
        }
        Ok(reconciled)
    }
}

#[cfg(test)]
mod tests {
    use crate::clock::{FixedClock, format_micros};
    use crate::db::Db;
    use crate::jobs::{Jobs, NewJob};
    use crate::library::LibraryRoots;
    use crate::media::Media;
    use crate::models::MediaMeta;
    use crate::repo::Ctx;
    use crate::schema::migrate;

    use super::{Recovery, RecoverySummary};

    const NOW: i64 = 1_700_000_000_000_000;

    fn fixture() -> (Db, crate::db::Conn, FixedClock, i64) {
        let database = Db::memory().unwrap();
        let mut conn = database.conn().unwrap();
        migrate(&mut conn).unwrap();
        let clock = FixedClock(NOW);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let root = LibraryRoots(ctx).create("library", "/library").unwrap();
        let media = Media(ctx)
            .create(
                root.id,
                "a.mp4",
                &MediaMeta {
                    kind: "video".to_owned(),
                    suffix: ".mp4".to_owned(),
                    size: 1,
                    mtime_ns: 1,
                    fingerprint: "fingerprint".to_owned(),
                    scanned_device: Some(1),
                    scanned_inode: Some(1),
                    scanned_ctime_ns: Some(1),
                    ..MediaMeta::default()
                },
            )
            .unwrap();
        let job = Jobs(ctx)
            .create(&NewJob::new("verify", &[media.id]))
            .unwrap();
        (database, conn, clock, job.id)
    }

    #[test]
    fn expired_item_is_interrupted_once_and_parent_is_settled() {
        let (_database, conn, clock, job_id) = fixture();
        let expired = format_micros(NOW - 1);
        conn.raw().execute(
            "UPDATE job_items SET status='running', executor_fencing_token=7, lease_expires_at=?1
             WHERE job_id=?2", rusqlite::params![expired, job_id],
        ).unwrap();
        let recovered = Recovery(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        assert_eq!(
            recovered.recover_expired_items(100).unwrap(),
            RecoverySummary {
                recovered: 1,
                reconciled_jobs: 1,
                retried: 0,
                failed: 0,
            }
        );
        assert_eq!(
            recovered.recover_expired_items(100).unwrap(),
            RecoverySummary::default()
        );
        let item: (String, String) = conn
            .raw()
            .query_row(
                "SELECT status, error_code FROM job_items WHERE job_id=?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            item,
            ("interrupted".to_owned(), "worker_stopped".to_owned())
        );
        let job: String = conn
            .raw()
            .query_row("SELECT status FROM jobs WHERE id=?1", [job_id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(job, "failed");
    }

    #[test]
    fn recovery_retries_only_non_destructive_work_while_gate_is_open() {
        let (_database, conn, clock, job_id) = fixture();
        conn.raw()
            .execute(
                "UPDATE job_items SET status='claimed', attempt=1, executor_fencing_token=7,
                    lease_expires_at=?1 WHERE job_id=?2",
                rusqlite::params![format_micros(NOW - 1), job_id],
            )
            .unwrap();
        let recovered = Recovery(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        let summary = recovered.recover_with_retries(100, 3).unwrap();
        assert_eq!(summary.retried, 1);
        let retry: (String, i64, i64) = conn
            .raw()
            .query_row(
                "SELECT jobs.trigger, job_items.attempt, job_items.origin_item_id
                 FROM job_items JOIN jobs ON jobs.id=job_items.job_id
                 WHERE job_items.job_id != ?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(retry, ("retry".to_owned(), 1, 1));

        conn.raw()
            .execute(
                "UPDATE deployment_gate SET mode='maintenance' WHERE id=1",
                [],
            )
            .unwrap();
        assert_eq!(recovered.recover_with_retries(100, 3).unwrap().retried, 0);
    }

    #[test]
    fn renewed_or_unexpired_work_is_not_recovered() {
        let (_database, conn, clock, job_id) = fixture();
        conn.raw().execute(
            "UPDATE job_items SET status='claimed', executor_fencing_token=7, lease_expires_at=?1
             WHERE job_id=?2", rusqlite::params![format_micros(NOW + 1), job_id],
        ).unwrap();
        let recovered = Recovery(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        assert_eq!(
            recovered.recover_expired_items(100).unwrap(),
            RecoverySummary::default()
        );
    }

    #[test]
    fn a_cancelled_job_with_only_queued_items_is_cancelled_by_reconciliation() {
        let (_database, conn, clock, _) = fixture();
        conn.raw()
            .execute_batch(
                "INSERT INTO jobs(id,action,status) VALUES(900,'verify','cancel_requested');
                 INSERT INTO job_items(job_id,media_file_id,action,position,status)
                   SELECT 900,id,'verify',1,'queued' FROM media_files LIMIT 1;",
            )
            .unwrap();
        let recovery = Recovery(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        assert_eq!(recovery.reconcile_idle_jobs().unwrap(), 1);
        let status = |sql: &str| {
            conn.raw()
                .query_row(sql, [], |row| row.get::<_, String>(0))
                .unwrap()
        };
        assert_eq!(status("SELECT status FROM jobs WHERE id=900"), "cancelled");
        assert_eq!(
            status("SELECT status FROM job_items WHERE job_id=900"),
            "cancelled"
        );
    }

    #[test]
    fn scans_without_a_live_lease_or_superseded_are_interrupted() {
        let (_database, conn, clock, _) = fixture();
        conn.raw()
            .execute_batch(
                "INSERT INTO scan_runs(id,root_id,generation,trigger,status,claimed_by,fencing_token)
                   SELECT 801,id,1,'scheduled','running','dead-owner',1 FROM library_roots LIMIT 1;",
            )
            .unwrap();
        let recovery = Recovery(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        recovery.recover_with_retries(100, 3).unwrap();
        let status: String = conn
            .raw()
            .query_row("SELECT status FROM scan_runs WHERE id=801", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(status, "interrupted");
    }
}

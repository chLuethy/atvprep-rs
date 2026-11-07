//! Bounded synchronous worker orchestration; async hosts call it in a blocking task.

use crate::Result;
use crate::db::Conn;
use crate::repo::Ctx;
use crate::schedules::run_due_schedules;
use crate::worker::maintenance::{MaintenanceLease, RetentionPolicy, RetentionSummary};
use crate::worker::recovery::{Recovery, RecoverySummary};

/// Limits for one worker pass, keeping every maintenance operation bounded.
#[derive(Debug, Clone, Copy)]
pub struct WorkerLimits {
    pub recovery_items: i64,
    pub recovery_attempts: i64,
    pub maintenance_batch: i64,
    pub event_retention_days: i64,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            recovery_items: 100,
            recovery_attempts: 3,
            maintenance_batch: 100,
            event_retention_days: 7,
        }
    }
}

/// Observable result of one bounded worker pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerTick {
    pub recovery: RecoverySummary,
    pub schedules_run: usize,
    pub events_pruned: usize,
    pub other_records_pruned: usize,
}

/// Coordinates durable, short-lived worker activities. Media scanning/execution remains owned by
/// its dedicated loop; this type performs only bounded database work.
pub struct WorkerService<'a> {
    ctx: Ctx<'a>,
    conn: &'a Conn,
}

impl<'a> WorkerService<'a> {
    /// Validate a claimed changing item immediately before opening its executor.
    ///
    /// # Errors
    /// Rejects lost claims, expired leases, changed inputs or invalid approvals.
    pub fn authorize_claimed_prepare(
        &self,
        item_id: i64,
        owner: &str,
        fence: i64,
        verifier: &crate::approval::ApprovalVerifier,
        identity: &[serde_json::Value; 4],
        idle_seconds: i64,
    ) -> Result<()> {
        let valid:bool=self.ctx.conn.query_row("SELECT EXISTS(SELECT 1 FROM job_items i JOIN jobs j ON j.id=i.job_id JOIN media_executor_leases l ON l.id=1 WHERE i.id=?1 AND i.claimed_by=?2 AND i.executor_fencing_token=?3 AND i.status IN ('claimed','running') AND i.lease_expires_at>?4 AND j.status IN ('queued','running') AND l.owner_id=?2 AND l.fencing_token=?3 AND l.lease_expires_at>?4)",rusqlite::params![item_id,owner,fence,self.ctx.clock.now()],|row|row.get(0))?;
        if !valid {
            return Err(crate::Error::Conflict(
                "worker claim is no longer current".to_owned(),
            ));
        }
        crate::worker::manual_policy::authorize_prepare(
            self.ctx,
            item_id,
            verifier,
            identity,
            idle_seconds,
        )
        .map(|_| ())
    }

    #[must_use]
    pub const fn new(ctx: Ctx<'a>, conn: &'a Conn) -> Self {
        Self { ctx, conn }
    }

    /// Execute one recovery, scheduling and maintenance pass.
    ///
    /// # Errors
    ///
    /// A durable repository operation fails. Losing the maintenance lease only skips cleanup.
    pub fn tick(&self, owner_id: &str, limits: WorkerLimits) -> Result<WorkerTick> {
        let recovery = Recovery(self.ctx)
            .recover_with_retries(limits.recovery_items, limits.recovery_attempts)?;
        let schedules_run = run_due_schedules(self.ctx)?;
        let summary = self.maintain(owner_id, limits)?;
        Ok(WorkerTick {
            recovery,
            schedules_run,
            events_pruned: summary.events,
            other_records_pruned: summary.authentication
                + summary.inspections
                + summary.jobs
                + summary.logs
                + summary.permits
                + summary.schedule_runs,
        })
    }

    fn maintain(&self, owner_id: &str, limits: WorkerLimits) -> Result<RetentionSummary> {
        let tx = rusqlite::Transaction::new_unchecked(
            self.ctx.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let ctx = Ctx {
            conn: &tx,
            clock: self.ctx.clock,
        };
        let days = |key: &str, fallback: i64| -> Result<i64> {
            crate::settings::Settings(ctx)
                .get(key)?
                .map_or(Ok(fallback), |setting| {
                    setting
                        .value
                        .as_i64()
                        .filter(|days| (1..=crate::settings::MAX_RETENTION_DAYS).contains(days))
                        .ok_or_else(|| {
                            crate::Error::Invalid("invalid retention setting".to_owned())
                        })
                })
        };
        let policy = RetentionPolicy {
            event_days: days("event_retention_days", limits.event_retention_days)?,
            log_days: days("log_retention_days", 7)?,
            job_days: days("job_retention_days", 7)?,
            ..RetentionPolicy::default()
        };
        let mut lease = MaintenanceLease::new(ctx);
        let summary = if lease.acquire(owner_id, 30)? {
            let summary = lease.prune(owner_id, self.conn, policy, limits.maintenance_batch)?;
            if !lease.held(owner_id)? || !lease.release(owner_id)? {
                return Err(crate::Error::Conflict(
                    "maintenance lease expired during cleanup".to_owned(),
                ));
            }
            summary
        } else {
            RetentionSummary::default()
        };
        tx.commit()?;
        Ok(summary)
    }
}

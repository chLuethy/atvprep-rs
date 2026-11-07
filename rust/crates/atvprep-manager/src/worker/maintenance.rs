//! Lease primitives for bounded, fenced maintenance work.

use rusqlite::OptionalExtension;

use crate::Result;
use crate::clock::format_micros;
use crate::repo::Ctx;
use crate::state_machine::JobStatus;

/// Retention grace periods after expiry or terminal completion, in days.
#[derive(Clone, Copy)]
pub struct RetentionPolicy {
    pub session_days: i64,
    pub idempotency_days: i64,
    pub event_days: i64,
    pub inspection_days: i64,
    pub log_days: i64,
    pub job_days: i64,
}
impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            session_days: 7,
            idempotency_days: 7,
            event_days: 7,
            inspection_days: 30,
            log_days: 30,
            job_days: 7,
        }
    }
}
impl RetentionPolicy {
    fn cutoff(now: i64, days: i64) -> String {
        format_micros(now.saturating_sub(days.saturating_mul(86_400_000_000)))
    }
    fn validate(self, batch: i64) -> Result<()> {
        if !(1..=1000).contains(&batch)
            || [
                self.session_days,
                self.idempotency_days,
                self.event_days,
                self.inspection_days,
                self.log_days,
                self.job_days,
            ]
            .iter()
            .any(|days| !(1..=3650).contains(days))
        {
            return Err(crate::Error::Invalid(
                "invalid retention policy or batch size".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Runs kept in the history of each schedule.
const SCHEDULE_RUNS_KEPT: i64 = 10;

#[derive(Default)]
pub struct RetentionSummary {
    pub events: usize,
    pub authentication: usize,
    pub inspections: usize,
    pub jobs: usize,
    pub logs: usize,
    pub permits: usize,
    pub schedule_runs: usize,
}

/// The database-global lease held by one maintenance worker.
pub struct MaintenanceLease<'a> {
    ctx: Ctx<'a>,
    /// The token acquired by this value, if it currently owns the lease.
    pub fencing_token: Option<i64>,
}

impl<'a> MaintenanceLease<'a> {
    /// Make a lease handle without claiming it.
    #[must_use]
    pub const fn new(ctx: Ctx<'a>) -> Self {
        Self {
            ctx,
            fencing_token: None,
        }
    }

    /// Acquire an expired or unheld lease and advance its fencing token.
    ///
    /// # Errors
    ///
    /// SQLite rejects the singleton insert or fenced update.
    pub fn acquire(&mut self, owner_id: &str, lease_seconds: i64) -> Result<bool> {
        let now = self.ctx.clock.now_micros();
        self.ctx.conn.execute(
            "INSERT OR IGNORE INTO maintenance_leases (id, fencing_token) VALUES (1, 0)",
            [],
        )?;
        let token: Option<i64> = self
            .ctx
            .conn
            .query_row(
                "UPDATE maintenance_leases SET owner_id=?1, lease_expires_at=?2, heartbeat_at=?3,
                     fencing_token=fencing_token+1
                 WHERE id=1 AND (owner_id IS NULL OR lease_expires_at IS NULL OR lease_expires_at <= ?3)
                 RETURNING fencing_token",
                rusqlite::params![
                    owner_id,
                    format_micros(now + lease_seconds * 1_000_000),
                    format_micros(now),
                ],
                |row| row.get(0),
            )
            .optional()?;
        self.fencing_token = token;
        Ok(token.is_some())
    }

    /// Confirm that this handle still owns a live lease at a destructive boundary.
    ///
    /// # Errors
    ///
    /// SQLite cannot read the lease row.
    pub fn held(&self, owner_id: &str) -> Result<bool> {
        let Some(token) = self.fencing_token else {
            return Ok(false);
        };
        let now = format_micros(self.ctx.clock.now_micros());
        Ok(self
            .ctx
            .conn
            .query_row(
                "SELECT 1 FROM maintenance_leases
             WHERE id=1 AND owner_id=?1 AND fencing_token=?2 AND lease_expires_at > ?3",
                rusqlite::params![owner_id, token, now],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Extend a lease owned by this handle.
    ///
    /// # Errors
    ///
    /// SQLite rejects the fenced update.
    pub fn heartbeat(&self, owner_id: &str, lease_seconds: i64) -> Result<bool> {
        let Some(token) = self.fencing_token else {
            return Ok(false);
        };
        let now = self.ctx.clock.now_micros();
        Ok(self.ctx.conn.execute(
            "UPDATE maintenance_leases SET heartbeat_at=?3, lease_expires_at=?4
             WHERE id=1 AND owner_id=?1 AND fencing_token=?2 AND lease_expires_at > ?3",
            rusqlite::params![
                owner_id,
                token,
                format_micros(now),
                format_micros(now + lease_seconds * 1_000_000),
            ],
        )? == 1)
    }

    /// Release this lease only while the original token is still current.
    ///
    /// # Errors
    ///
    /// SQLite rejects the fenced update.
    pub fn release(&self, owner_id: &str) -> Result<bool> {
        let Some(token) = self.fencing_token else {
            return Ok(false);
        };
        Ok(self.ctx.conn.execute(
            "UPDATE maintenance_leases SET owner_id=NULL, lease_expires_at=NULL, heartbeat_at=NULL
             WHERE id=1 AND owner_id=?1 AND fencing_token=?2",
            rusqlite::params![owner_id, token],
        )? == 1)
    }

    /// Delete at most `batch_size` expired events using the connection-scoped retention capability.
    ///
    /// The lease is checked before selecting candidates and again before each irreversible delete.
    /// A lost lease stops the pass without touching later events.
    ///
    /// # Errors
    ///
    /// `batch_size` is not positive or SQLite rejects a query/delete.
    pub fn prune_expired_events(
        &self,
        owner_id: &str,
        conn: &crate::db::Conn,
        batch_size: i64,
    ) -> Result<usize> {
        self.prune_events_before(owner_id, conn, batch_size, &self.ctx.clock.now())
    }

    fn prune_events_before(
        &self,
        owner_id: &str,
        conn: &crate::db::Conn,
        batch_size: i64,
        cutoff: &str,
    ) -> Result<usize> {
        if batch_size < 1 {
            return Err(crate::Error::Invalid(
                "batch_size must be positive".to_owned(),
            ));
        }
        if !self.held(owner_id)? {
            return Ok(0);
        }
        let mut statement = self
            .ctx
            .conn
            .prepare("SELECT id FROM events WHERE expires_at <= ?1 ORDER BY id LIMIT ?2")?;
        let ids = statement
            .query_map(rusqlite::params![cutoff, batch_size], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let mut deleted = 0;
        for id in ids {
            if !self.held(owner_id)? {
                break;
            }
            deleted += conn.delete_event_for_retention(id)?;
        }
        Ok(deleted)
    }

    /// Delete old inspections without deleting the newest one for any media file.
    ///
    /// # Errors
    ///
    /// The lease is lost, a limit is invalid, or SQLite rejects the bounded deletion.
    pub fn prune_inspections(
        &self,
        owner_id: &str,
        batch_size: i64,
        keep_days: i64,
    ) -> Result<usize> {
        if !(1..=1000).contains(&batch_size) || !(1..=3650).contains(&keep_days) {
            return Err(crate::Error::Invalid(
                "retention limits must be positive".to_owned(),
            ));
        }
        if !self.held(owner_id)? {
            return Ok(0);
        }
        let cutoff = format_micros(self.ctx.clock.now_micros() - keep_days * 86_400_000_000);
        let mut statement = self.ctx.conn.prepare(
            "SELECT old.id FROM inspections AS old
             WHERE old.created_at < ?1 AND old.id != (
                 SELECT newest.id FROM inspections AS newest
                 WHERE newest.media_file_id=old.media_file_id
                 ORDER BY newest.created_at DESC, newest.id DESC LIMIT 1
             ) ORDER BY old.id LIMIT ?2",
        )?;
        let ids = statement
            .query_map(rusqlite::params![cutoff, batch_size], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let mut deleted = 0;
        for id in ids {
            if !self.held(owner_id)? {
                break;
            }
            deleted += self
                .ctx
                .conn
                .execute("DELETE FROM inspections WHERE id=?1", [id])?;
        }
        Ok(deleted)
    }

    /// Delete at most `batch_size` old terminal jobs after their events and dependent retries are gone.
    ///
    /// # Errors
    ///
    /// `batch_size` is not positive or SQLite rejects a fenced delete.
    pub fn prune_terminal_jobs(
        &self,
        owner_id: &str,
        batch_size: i64,
        keep_days: i64,
    ) -> Result<usize> {
        if !(1..=1000).contains(&batch_size) || !(1..=3650).contains(&keep_days) {
            return Err(crate::Error::Invalid(
                "retention limits must be positive".to_owned(),
            ));
        }
        if !self.held(owner_id)? {
            return Ok(0);
        }
        let cutoff = format_micros(self.ctx.clock.now_micros() - keep_days * 86_400_000_000);
        let mut statement = self.ctx.conn.prepare(
            &format!("SELECT jobs.id FROM jobs
             WHERE status IN {}
               AND finished_at IS NOT NULL AND finished_at < ?1
               AND NOT EXISTS (SELECT 1 FROM events WHERE events.job_id=jobs.id)
               AND NOT EXISTS (SELECT 1 FROM jobs AS retry_job WHERE retry_job.origin_job_id=jobs.id)
               AND NOT EXISTS (SELECT 1 FROM events WHERE events.item_id IN (SELECT id FROM job_items WHERE job_id=jobs.id))
               AND NOT EXISTS (SELECT 1 FROM job_items AS retry
                               WHERE retry.origin_item_id IN (SELECT id FROM job_items WHERE job_id=jobs.id))
             ORDER BY jobs.id LIMIT ?2", JobStatus::terminal_sql_list()),
        )?;
        let ids = statement
            .query_map(rusqlite::params![cutoff, batch_size], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let mut deleted = 0;
        for id in ids {
            if !self.held(owner_id)? {
                break;
            }
            self.ctx.conn.execute("DELETE FROM input_snapshots WHERE job_item_id IN (SELECT id FROM job_items WHERE job_id=?1)", [id])?;
            self.ctx
                .conn
                .execute("DELETE FROM job_items WHERE job_id=?1", [id])?;
            deleted += self
                .ctx
                .conn
                .execute("DELETE FROM jobs WHERE id=?1", [id])?;
        }
        Ok(deleted)
    }

    fn expired(
        &self,
        owner_id: &str,
        table: &str,
        column: &str,
        cutoff: &str,
        batch: i64,
    ) -> Result<usize> {
        let Some(token) = self.fencing_token else {
            return Ok(0);
        };
        // Identifiers are fixed private call-site constants, never user input.
        Ok(self.ctx.conn.execute(&format!("DELETE FROM {table} WHERE id IN (SELECT id FROM {table} WHERE {column} < ?1 ORDER BY id LIMIT ?2) AND EXISTS(SELECT 1 FROM maintenance_leases WHERE id=1 AND owner_id=?3 AND fencing_token=?4 AND lease_expires_at>?5)"),rusqlite::params![cutoff,batch,owner_id,token,self.ctx.clock.now()])?)
    }

    /// Clean each disposable category in bounded batches under the current lease.
    /// Call within an immediate transaction to make the complete pass atomic.
    ///
    /// # Errors
    /// Rejects invalid limits, lost storage or failed fenced deletions.
    pub fn prune(
        &self,
        owner_id: &str,
        conn: &crate::db::Conn,
        policy: RetentionPolicy,
        batch: i64,
    ) -> Result<RetentionSummary> {
        policy.validate(batch)?;
        if !self.held(owner_id)? {
            return Ok(RetentionSummary::default());
        }
        let now = self.ctx.clock.now_micros();
        let sessions = self.expired(
            owner_id,
            "auth_sessions",
            "expires_at",
            &RetentionPolicy::cutoff(now, policy.session_days),
            batch,
        )?;
        let idempotency = self.expired(
            owner_id,
            "idempotency_records",
            "expires_at",
            &RetentionPolicy::cutoff(now, policy.idempotency_days),
            batch,
        )?;
        let csrf = self.expired(
            owner_id,
            "csrf_tokens",
            "expires_at",
            &RetentionPolicy::cutoff(now, policy.session_days),
            batch,
        )?;
        let events = self.prune_events_before(
            owner_id,
            conn,
            batch,
            &RetentionPolicy::cutoff(now, policy.event_days),
        )?;
        let inspections = self.prune_inspections(owner_id, batch, policy.inspection_days)?;
        let logs = if self.held(owner_id)? {
            self.ctx.conn.execute(&format!("UPDATE job_items SET log_tail=NULL WHERE id IN (SELECT i.id FROM job_items i JOIN jobs j ON j.id=i.job_id WHERE j.finished_at<?1 AND j.status IN {} AND i.log_tail IS NOT NULL ORDER BY i.id LIMIT ?2)",JobStatus::terminal_sql_list()),rusqlite::params![RetentionPolicy::cutoff(now,policy.log_days),batch])?
        } else {
            0
        };
        let jobs = self.prune_terminal_jobs(owner_id, batch, policy.job_days)?;
        let permits = if self.held(owner_id)? {
            crate::deployment_gate::DeploymentGate(self.ctx).prune_permits(7, batch)?
        } else {
            0
        };
        let schedule_runs = if self.held(owner_id)? {
            self.ctx.conn.execute(
                "DELETE FROM schedule_runs WHERE id IN (
                   SELECT id FROM (SELECT id, row_number() OVER (PARTITION BY schedule_id ORDER BY id DESC) AS rank
                                   FROM schedule_runs)
                   WHERE rank > ?1 ORDER BY id LIMIT ?2)",
                rusqlite::params![SCHEDULE_RUNS_KEPT, batch],
            )?
        } else {
            0
        };
        Ok(RetentionSummary {
            events,
            authentication: sessions + idempotency + csrf,
            inspections,
            jobs,
            logs,
            permits,
            schedule_runs,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::clock::FixedClock;
    use crate::db::Db;
    use crate::repo::Ctx;
    use crate::schema::migrate;

    use super::MaintenanceLease;

    #[test]
    fn only_one_owner_holds_a_live_lease_and_stale_tokens_cannot_release() {
        let database = Db::memory().unwrap();
        let mut conn = database.conn().unwrap();
        migrate(&mut conn).unwrap();
        let early = FixedClock(1_000_000);
        let later = FixedClock(33_000_000);
        let mut first = MaintenanceLease::new(Ctx {
            conn: conn.raw(),
            clock: &early,
        });
        assert!(first.acquire("first", 30).unwrap());
        assert!(first.held("first").unwrap());
        let mut second = MaintenanceLease::new(Ctx {
            conn: conn.raw(),
            clock: &early,
        });
        assert!(!second.acquire("second", 30).unwrap());
        let mut takeover = MaintenanceLease::new(Ctx {
            conn: conn.raw(),
            clock: &later,
        });
        assert!(takeover.acquire("second", 30).unwrap());
        assert!(!first.heartbeat("first", 30).unwrap());
        assert!(!first.release("first").unwrap());
        assert!(takeover.held("second").unwrap());
        assert!(takeover.release("second").unwrap());
    }

    #[test]
    fn terminal_job_retention_keeps_jobs_with_events() {
        let database = Db::memory().unwrap();
        let mut conn = database.conn().unwrap();
        migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        conn.raw().execute_batch(
            "INSERT INTO jobs(action, status, finished_at) VALUES ('verify', 'failed', '2023-11-01 00:00:00.000000');
             INSERT INTO jobs(action, status, finished_at) VALUES ('verify', 'failed', '2023-11-01 00:00:00.000000');
             INSERT INTO events(job_id, topic, entity_type, entity_id, payload_json)
             VALUES (2, 'kept', 'job', 2, '{}');",
        ).unwrap();
        let mut lease = MaintenanceLease::new(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        assert!(lease.acquire("worker", 30).unwrap());
        assert_eq!(lease.prune_terminal_jobs("worker", 10, 7).unwrap(), 1);
        let ids: Vec<i64> = conn
            .raw()
            .prepare("SELECT id FROM jobs ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(ids, [2]);
    }

    #[test]
    fn event_retention_is_bounded_and_requires_the_current_lease() {
        let database = Db::memory().unwrap();
        let mut conn = database.conn().unwrap();
        migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        conn.raw()
            .execute_batch(
                "INSERT INTO events(topic, entity_type, entity_id, payload_json, expires_at)
             VALUES ('old', 'job', 1, '{}', '2023-11-01 00:00:00.000000');
             INSERT INTO events(topic, entity_type, entity_id, payload_json, expires_at)
             VALUES ('old', 'job', 2, '{}', '2023-11-01 00:00:00.000000');
             INSERT INTO events(topic, entity_type, entity_id, payload_json, expires_at)
             VALUES ('new', 'job', 3, '{}', '2024-01-01 00:00:00.000000');",
            )
            .unwrap();
        let mut lease = MaintenanceLease::new(Ctx {
            conn: conn.raw(),
            clock: &clock,
        });
        assert_eq!(lease.prune_expired_events("worker", &conn, 1).unwrap(), 0);
        assert!(lease.acquire("worker", 30).unwrap());
        assert_eq!(lease.prune_expired_events("worker", &conn, 1).unwrap(), 1);
        let remaining: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 2);
        assert!(lease.prune_expired_events("worker", &conn, 0).is_err());
    }
}

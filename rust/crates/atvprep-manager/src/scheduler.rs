//! Database-backed ownership of the single automatic scan scheduler.

use rusqlite::OptionalExtension;

use crate::clock::format_micros;
use crate::models::ScanRun;
use crate::repo::Ctx;
use crate::{Error, Result};

pub struct SchedulerLease<'a> {
    ctx: Ctx<'a>,
    pub fencing_token: Option<i64>,
}

impl<'a> SchedulerLease<'a> {
    #[must_use]
    pub const fn new(ctx: Ctx<'a>) -> Self {
        Self {
            ctx,
            fencing_token: None,
        }
    }

    /// Take or renew the singleton lease. Each successful acquisition gets a
    /// new fencing token, including a renewal by the same owner.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn acquire(
        &mut self,
        owner_id: &str,
        lease_seconds: i64,
        interval_seconds: i64,
    ) -> Result<bool> {
        let now = self.ctx.clock.now_micros();
        self.ctx.conn.execute(
            "INSERT OR IGNORE INTO scheduler_leases (id, interval_seconds, fencing_token) VALUES (1, ?1, 0)",
            [interval_seconds],
        )?;
        let token = self.ctx.conn.query_row(
            "UPDATE scheduler_leases SET owner_id=?1, lease_expires_at=?2, heartbeat_at=?3,
                 interval_seconds=?4, fencing_token=fencing_token+1
             WHERE id=1 AND (owner_id IS NULL OR lease_expires_at IS NULL OR lease_expires_at <= ?3 OR owner_id=?1)
             RETURNING fencing_token",
            rusqlite::params![owner_id, format_micros(now + lease_seconds * 1_000_000), format_micros(now), interval_seconds],
            |row| row.get(0),
        ).optional()?;
        self.fencing_token = token;
        Ok(token.is_some())
    }

    /// Extend a current lease only while its owner, token and expiry still match.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn heartbeat(
        &self,
        owner_id: &str,
        token: Option<i64>,
        lease_seconds: i64,
    ) -> Result<bool> {
        let Some(token) = token.or(self.fencing_token) else {
            return Ok(false);
        };
        let now = self.ctx.clock.now_micros();
        Ok(self.ctx.conn.execute(
            "UPDATE scheduler_leases SET heartbeat_at=?3, lease_expires_at=?4
             WHERE id=1 AND owner_id=?1 AND fencing_token=?2 AND lease_expires_at > ?3",
            rusqlite::params![
                owner_id,
                token,
                format_micros(now),
                format_micros(now + lease_seconds * 1_000_000)
            ],
        )? == 1)
    }

    /// Update the automatic interval while this lease remains current.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn update_interval(&self, owner_id: &str, interval_seconds: i64) -> Result<bool> {
        let Some(token) = self.fencing_token else {
            return Ok(false);
        };
        let now = self.ctx.clock.now();
        Ok(self.ctx.conn.execute(
            "UPDATE scheduler_leases SET interval_seconds=?3
             WHERE id=1 AND owner_id=?1 AND fencing_token=?2 AND lease_expires_at > ?4",
            rusqlite::params![owner_id, token, interval_seconds, now],
        )? == 1)
    }

    /// Release exactly this acquired lease.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn release(&self, owner_id: &str) -> Result<bool> {
        let Some(token) = self.fencing_token else {
            return Ok(false);
        };
        Ok(self.ctx.conn.execute(
            "UPDATE scheduler_leases SET owner_id=NULL, lease_expires_at=NULL, heartbeat_at=NULL
             WHERE id=1 AND owner_id=?1 AND fencing_token=?2",
            rusqlite::params![owner_id, token],
        )? == 1)
    }
}

pub struct Scheduler<'a> {
    ctx: Ctx<'a>,
    owner_id: &'a str,
    lease_seconds: i64,
    pub lease: SchedulerLease<'a>,
}

impl<'a> Scheduler<'a> {
    /// Release this scheduler's fence, retrying transient locks a bounded number of times.
    /// Shutdown remains quiet when the database stays unavailable; the lease then expires.
    pub fn shutdown(&self, attempts: u32) {
        let attempts = attempts.min(8);
        for attempt in 0..attempts {
            match self.lease.release(self.owner_id) {
                Err(Error::Busy) if attempt + 1 < attempts => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                _ => break,
            }
        }
    }

    #[must_use]
    pub const fn new(ctx: Ctx<'a>, owner_id: &'a str, lease_seconds: i64) -> Self {
        Self {
            ctx,
            owner_id,
            lease_seconds,
            lease: SchedulerLease::new(ctx),
        }
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn acquire(&mut self, interval_seconds: i64) -> Result<bool> {
        self.lease
            .acquire(self.owner_id, self.lease_seconds, interval_seconds)
    }

    /// Create one fenced, durable scheduled scan for a root/generation pair.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn create_scheduled_scan(&self, root_id: i64, generation: i64) -> Result<bool> {
        let Some(token) = self.lease.fencing_token else {
            return Ok(false);
        };
        let now = self.ctx.clock.now();
        Ok(self.ctx.conn.execute(
            "INSERT INTO scan_runs(root_id, generation, trigger, status, claimed_by, fencing_token, heartbeat_at, lease_expires_at, started_at)
             SELECT ?1, ?2, 'scheduled', 'running', owner_id, fencing_token, heartbeat_at, lease_expires_at, ?3
             FROM scheduler_leases WHERE id=1 AND owner_id=?4 AND fencing_token=?5 AND lease_expires_at > ?3
               AND EXISTS(SELECT 1 FROM deployment_gate WHERE id=1 AND mode='open' AND phase='steady')
             ON CONFLICT(root_id, generation) DO NOTHING",
            rusqlite::params![root_id, generation, now, self.owner_id, token],
        )? == 1)
    }

    /// Claim the oldest manual scan under the scheduler's current fence.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn claim_manual_scan(&self) -> Result<Option<ScanRun>> {
        let Some(token) = self.lease.fencing_token else {
            return Ok(None);
        };
        let now = self.ctx.clock.now();
        let id: Option<i64> = self.ctx.conn.query_row(
            "UPDATE scan_runs SET status='running', claimed_by=?1, fencing_token=?2,
                 heartbeat_at=(SELECT heartbeat_at FROM scheduler_leases WHERE id=1),
                 lease_expires_at=(SELECT lease_expires_at FROM scheduler_leases WHERE id=1)
             WHERE id=(SELECT id FROM scan_runs WHERE trigger='manual' AND status='queued' ORDER BY id LIMIT 1)
               AND EXISTS (SELECT 1 FROM scheduler_leases WHERE id=1 AND owner_id=?1 AND fencing_token=?2 AND lease_expires_at > ?3)
               AND EXISTS(SELECT 1 FROM deployment_gate WHERE id=1 AND mode='open' AND phase='steady')
             RETURNING id",
            rusqlite::params![self.owner_id, token, now], |row| row.get(0),
        ).optional()?;
        id.map(|id| {
            self.ctx
                .conn
                .query_row(
                    "SELECT * FROM scan_runs WHERE id=?1",
                    [id],
                    ScanRun::from_row,
                )
                .map_err(Error::from)
        })
        .transpose()
    }
}

#[cfg(test)]
mod tests {
    use crate::clock::FixedClock;
    use crate::db::Db;
    use crate::library::LibraryRoots;
    use crate::repo::Ctx;
    use crate::schema;

    use super::*;

    fn setup() -> (Db, FixedClock) {
        (Db::memory().unwrap(), FixedClock(1_000_000))
    }

    #[test]
    fn lease_fences_stale_owners_and_releases() {
        let (db, clock) = setup();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let mut first = SchedulerLease::new(ctx);
        assert!(first.acquire("one", 10, 60).unwrap());
        let old = first.fencing_token;
        let late = FixedClock(12_000_000);
        let mut second = SchedulerLease::new(Ctx {
            conn: conn.raw(),
            clock: &late,
        });
        assert!(second.acquire("two", 10, 60).unwrap());
        assert!(!first.heartbeat("one", old, 10).unwrap());
        assert!(!first.release("one").unwrap());
        assert!(second.release("two").unwrap());
    }

    #[test]
    fn scheduled_scans_are_unique_and_fenced() {
        let (db, clock) = setup();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let root = LibraryRoots(Ctx {
            conn: conn.raw(),
            clock: &clock,
        })
        .create("root", "/media")
        .unwrap();
        let mut scheduler = Scheduler::new(
            Ctx {
                conn: conn.raw(),
                clock: &clock,
            },
            "worker",
            30,
        );
        assert!(scheduler.acquire(60).unwrap());
        assert!(scheduler.create_scheduled_scan(root.id, 1).unwrap());
        assert!(!scheduler.create_scheduled_scan(root.id, 1).unwrap());
        let taken = FixedClock(32_000_000);
        let mut other = SchedulerLease::new(Ctx {
            conn: conn.raw(),
            clock: &taken,
        });
        assert!(other.acquire("other", 30, 60).unwrap());
        assert!(!scheduler.create_scheduled_scan(root.id, 2).unwrap());
    }
}

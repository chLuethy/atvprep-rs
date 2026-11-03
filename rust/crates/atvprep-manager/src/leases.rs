//! The database-global lease of media-executing workers (verify, fix, convert).
//!
//! Exactly one worker holds it at a time. Its monotonic fencing token is stamped on every item it
//! claims, so losing the lease (a failed heartbeat, a crash) invalidates late commits of the
//! stale worker.

use rusqlite::OptionalExtension;

use crate::Result;
use crate::clock::format_micros;
use crate::repo::Ctx;

pub struct MediaExecutorLease<'a> {
    ctx: Ctx<'a>,
    /// The token of the lease this value acquired, once it has.
    pub fencing_token: Option<i64>,
}

impl<'a> MediaExecutorLease<'a> {
    #[must_use]
    pub const fn new(ctx: Ctx<'a>) -> Self {
        Self {
            ctx,
            fencing_token: None,
        }
    }

    /// Try to take the lease for `owner_id`. On success `fencing_token` holds the new token; on
    /// failure another worker holds a lease that has not expired.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn acquire(&mut self, owner_id: &str, lease_seconds: i64) -> Result<bool> {
        let now = self.ctx.clock.now_micros();
        self.ctx.conn.execute(
            "INSERT OR IGNORE INTO media_executor_leases (id, fencing_token) VALUES (1, 0)",
            [],
        )?;
        let token: Option<i64> = self
            .ctx
            .conn
            .query_row(
                "UPDATE media_executor_leases SET owner_id=?1, lease_expires_at=?2, heartbeat_at=?3,
                     fencing_token=fencing_token+1
                 WHERE id=1 AND (owner_id IS NULL OR lease_expires_at IS NULL OR lease_expires_at <= ?3)
                 RETURNING fencing_token",
                rusqlite::params![
                    owner_id,
                    format_micros(now + lease_seconds * 1_000_000),
                    format_micros(now)
                ],
                |r| r.get(0),
            )
            .optional()?;
        self.fencing_token = token;
        Ok(token.is_some())
    }

    /// Extend the lease while it is still held under this fencing token (the given one, else the
    /// one acquired).
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn heartbeat(&self, fencing_token: Option<i64>, lease_seconds: i64) -> Result<bool> {
        let Some(token) = fencing_token.or(self.fencing_token) else {
            return Ok(false);
        };
        let now = self.ctx.clock.now_micros();
        Ok(self.ctx.conn.execute(
            "UPDATE media_executor_leases SET heartbeat_at=?2, lease_expires_at=?3
             WHERE id=1 AND fencing_token=?1 AND lease_expires_at > ?2",
            rusqlite::params![
                token,
                format_micros(now),
                format_micros(now + lease_seconds * 1_000_000)
            ],
        )? == 1)
    }

    /// Give the lease back if it is still held under this value's fencing token.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn release(&self) -> Result<bool> {
        let Some(token) = self.fencing_token else {
            return Ok(false);
        };
        Ok(self.ctx.conn.execute(
            "UPDATE media_executor_leases SET owner_id=NULL, lease_expires_at=NULL, heartbeat_at=NULL
             WHERE id=1 AND fencing_token=?1",
            [token],
        )? == 1)
    }
}

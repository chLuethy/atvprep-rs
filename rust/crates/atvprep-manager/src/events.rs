//! The append-only event log of jobs and their items.

use serde_json::Value;

use crate::clock::format_micros;
use crate::models::Event;
use crate::repo::Ctx;
use crate::{Error, Result};

/// How long an event is kept before retention may delete it.
const RETENTION_MICROS: i64 = 7 * 86_400 * 1_000_000;

pub struct Events<'a>(pub Ctx<'a>);

impl Events<'_> {
    /// # Errors
    ///
    /// The job is unknown.
    pub fn append(
        &self,
        job_id: i64,
        topic: &str,
        entity_type: &str,
        entity_id: i64,
        payload: &Value,
        item_id: Option<i64>,
    ) -> Result<Event> {
        let known: Option<i64> = rusqlite::OptionalExtension::optional(self.0.conn.query_row(
            "SELECT id FROM jobs WHERE id = ?1",
            [job_id],
            |r| r.get(0),
        ))?;
        if known.is_none() {
            return Err(Error::Invalid(format!("unknown job id: {job_id}")));
        }
        let now = self.0.clock.now_micros();
        self.0.conn.execute(
            "INSERT INTO events(job_id, item_id, topic, entity_type, entity_id, payload_json,
                                created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                job_id,
                item_id,
                topic,
                entity_type,
                entity_id,
                payload.to_string(),
                format_micros(now),
                format_micros(now + RETENTION_MICROS),
            ],
        )?;
        Ok(self.0.conn.query_row(
            "SELECT * FROM events WHERE id = ?1",
            [self.0.conn.last_insert_rowid()],
            Event::from_row,
        )?)
    }

    /// The events of a job in order, after `after` when given, at most `limit` (capped at 500).
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn list(&self, job_id: i64, after: Option<i64>, limit: i64) -> Result<Vec<Event>> {
        let mut statement = self
            .0
            .conn
            .prepare("SELECT * FROM events WHERE job_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3")?;
        let rows = statement.query_map(
            rusqlite::params![job_id, after.unwrap_or(i64::MIN), limit.min(500)],
            Event::from_row,
        )?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

//! Incremental, fenced progress and bounded log events from a running Prepare child.
use super::{
    executor::FfmpegProgress,
    logs::{ITEM_LOG_LIMIT, ItemLog},
};
use crate::{Error, Result, events::Events, jobs::Jobs, repo::Ctx};
use std::time::{Duration, Instant};

struct NonblockingWrite<'a> {
    conn: &'a rusqlite::Connection,
    previous: Duration,
}
impl<'a> NonblockingWrite<'a> {
    fn new(conn: &'a rusqlite::Connection) -> Result<Self> {
        let millis: i64 = conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
        conn.busy_timeout(Duration::ZERO)?;
        Ok(Self {
            conn,
            previous: Duration::from_millis(u64::try_from(millis).unwrap_or_default()),
        })
    }
}
impl Drop for NonblockingWrite<'_> {
    fn drop(&mut self) {
        let _ = self.conn.busy_timeout(self.previous);
    }
}

/// How much of the log a `job.log` event carries. The event tells live readers that the log moved;
/// the whole log is read from the item.
const EVENT_TAIL_LIMIT: usize = 8192;

/// The fenced sink of one running item: progress, step lines and tool output, written to the item
/// (`progress`, `log_tail`) and announced as `job.progress` and `job.log`.
///
/// - [`Self::feed`] takes tool output and parses progress from it.
/// - [`Self::step`] adds a stamped step line of the manager (see [`ItemLog`]).
/// - [`Self::finish`] writes the final state, waiting for the database if it must.
///
/// Every write needs the item's claim and lease; a lost authority writes nothing and is an error.
/// Writes only land while the item is running, so the last line must come before its terminal
/// status.
pub(super) struct ClaimedOutput<'a> {
    pub ctx: Ctx<'a>,
    pub owner: &'a str,
    pub fence: i64,
    pub item: i64,
    pub job: i64,
    parser: FfmpegProgress,
    log: ItemLog,
    progress: Option<u8>,
    flushed: Option<Instant>,
}
impl<'a> ClaimedOutput<'a> {
    pub fn new(ctx: Ctx<'a>, owner: &'a str, fence: i64, item: i64, job: i64) -> Self {
        Self {
            ctx,
            owner,
            fence,
            item,
            job,
            parser: FfmpegProgress::new(),
            log: ItemLog::new(ITEM_LOG_LIMIT),
            progress: None,
            flushed: None,
        }
    }

    /// Take a chunk of tool output. Writes at most every 250 ms unless `force` or progress rose; a
    /// busy database skips the write instead of delaying the tool.
    pub fn feed(&mut self, chunk: &[u8], force: bool) -> Result<()> {
        self.log.tool(chunk);
        let increased = self.parser.feed(chunk);
        if let Some(percent) = increased {
            self.progress = Some(percent);
        }
        if !force
            && increased.is_none()
            && self
                .flushed
                .is_some_and(|when| when.elapsed() < Duration::from_millis(250))
        {
            return Ok(());
        }
        self.flush(false)
    }

    /// Add a step line, stamped with the current time, and write it without waiting.
    pub fn step(&mut self, line: &str) -> Result<()> {
        self.log.step(self.ctx.clock, line);
        self.flush(false)
    }

    /// Write everything still pending, waiting for a busy database like any other write.
    pub fn finish(&mut self) -> Result<()> {
        self.flush(true)
    }

    /// Write the log after the lease was lost, so that the cause of the end is not lost with it.
    ///
    /// The one write left to a worker without authority: only the log text, and only to the item
    /// that still carries this worker's fencing token and is still running. A worker that was
    /// replaced finds the item changed and writes nothing. No progress and no event follow.
    pub fn finish_without_authority(&mut self) -> Result<()> {
        if self.log.is_empty() {
            return Ok(());
        }
        let text = self.log.text();
        crate::db::retry_busy(|| Jobs(self.ctx).set_item_log_tail(self.item, self.fence, &text))?;
        Ok(())
    }

    fn flush(&mut self, wait: bool) -> Result<()> {
        let nonblocking = if wait {
            None
        } else {
            Some(NonblockingWrite::new(self.ctx.conn)?)
        };
        let begin = || {
            rusqlite::Transaction::new_unchecked(
                self.ctx.conn,
                rusqlite::TransactionBehavior::Immediate,
            )
            .map_err(Error::from)
        };
        let tx = match if wait {
            crate::db::retry_busy(begin)
        } else {
            begin()
        } {
            Ok(tx) => tx,
            Err(Error::Busy) if !wait => {
                self.flushed = Some(Instant::now());
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        drop(nonblocking);
        let ctx = Ctx {
            conn: &tx,
            clock: self.ctx.clock,
        };
        let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM job_items i JOIN media_executor_leases l ON l.id=1 WHERE i.id=?1 AND i.claimed_by=?2 AND i.executor_fencing_token=?3 AND i.lease_expires_at>?4 AND l.owner_id=?2 AND l.fencing_token=?3 AND l.lease_expires_at>?4)",rusqlite::params![self.item,self.owner,self.fence,ctx.clock.now()],|row|row.get(0))?;
        if !live {
            return Err(Error::Conflict(
                "worker output authority expired".to_owned(),
            ));
        }
        if let Some(percent) = self.progress
            && Jobs(ctx).set_item_progress(self.item, self.fence, i64::from(percent))?
        {
            Events(ctx).append(
                self.job,
                "job.progress",
                "job_item",
                self.item,
                &serde_json::json!({"percent":percent}),
                Some(self.item),
            )?;
        }
        if !self.log.is_empty() {
            let text = self.log.text();
            if Jobs(ctx).set_item_log_tail(self.item, self.fence, &text)? {
                let mut start = text.len().saturating_sub(EVENT_TAIL_LIMIT);
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                Events(ctx).append(
                    self.job,
                    "job.log",
                    "job_item",
                    self.item,
                    &serde_json::json!({"tail":&text[start..]}),
                    Some(self.item),
                )?;
            }
        }
        tx.commit()?;
        self.flushed = Some(Instant::now());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{clock::FixedClock, db::Db, schema};
    fn seed(db: &Db) -> crate::db::Conn {
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        conn.raw().execute_batch("INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'test','/library');
        INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,discovery_state,compliance_state,workflow_state,desired_action) VALUES(1,1,'test.mp4','video','.mp4',1,1,'test','present','unknown','running','fix');
        INSERT INTO jobs(id,action,status) VALUES(1,'fix','running');
        INSERT INTO job_items(id,job_id,media_file_id,action,position,status,claimed_by,executor_fencing_token,lease_expires_at) VALUES(1,1,1,'fix',1,'running','worker',7,'2999-01-01');
        UPDATE media_executor_leases SET owner_id='worker',fencing_token=7,lease_expires_at='2999-01-01' WHERE id=1;").unwrap();
        conn
    }

    #[test]
    fn a_busy_output_sink_does_not_delay_execution_or_lose_the_pending_percent() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("worker.sqlite")).unwrap();
        let conn = seed(&db);
        let blocker = db.conn().unwrap();
        blocker.raw().execute_batch("BEGIN IMMEDIATE").unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let mut output = ClaimedOutput::new(
            Ctx {
                conn: conn.raw(),
                clock: &clock,
            },
            "worker",
            7,
            1,
            1,
        );
        let started = Instant::now();
        output.feed(b"ATVPREP-PROGRESS 25\n", true).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            conn.raw()
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            5000
        );
        blocker.raw().execute_batch("ROLLBACK").unwrap();
        output.feed(&[], true).unwrap();
        assert_eq!(
            conn.raw()
                .query_row("SELECT progress FROM job_items WHERE id=1", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            25
        );
    }

    #[test]
    fn progress_is_monotone_logs_are_bounded_and_lost_authority_writes_nothing() {
        let db = Db::memory().unwrap();
        let conn = seed(&db);
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let mut output = ClaimedOutput::new(ctx, "worker", 7, 1, 1);
        for percent in [25, 10, 75] {
            output
                .feed(format!("ATVPREP-PROGRESS {percent}\n").as_bytes(), true)
                .unwrap();
        }
        output.feed(b"HEAD\n", true).unwrap();
        output.feed(&vec![b'x'; 200_000], true).unwrap();
        output.feed(b"\nEND\n", true).unwrap();
        let (percent, tail): (i64, String) = conn
            .raw()
            .query_row(
                "SELECT progress,log_tail FROM job_items WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(percent, 75);
        assert!(tail.len() <= 64 * 1024);
        assert!(tail.contains("ATVPREP-PROGRESS 25\nATVPREP-PROGRESS 10\n"));
        assert!(tail.contains("bytes omitted"));
        assert!(tail.ends_with("\nEND\n"));
        // The live event keeps its former size: it says the log moved, the log itself is read.
        let event: String = conn
            .raw()
            .query_row(
                "SELECT payload_json FROM events WHERE topic='job.log' ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let event: serde_json::Value = serde_json::from_str(&event).unwrap();
        let event = event["tail"].as_str().unwrap();
        assert!(event.len() <= 8192);
        assert!(event.ends_with("\nEND\n"));
        let values: Vec<String> = conn
            .raw()
            .prepare("SELECT payload_json FROM events WHERE topic='job.progress' ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        let values: Vec<serde_json::Value> = values
            .iter()
            .map(|value| serde_json::from_str(value).unwrap())
            .collect();
        assert_eq!(
            values,
            vec![
                serde_json::json!({"percent":25}),
                serde_json::json!({"percent":75})
            ]
        );
        let before: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .unwrap();
        conn.raw()
            .execute(
                "UPDATE media_executor_leases SET fencing_token=8 WHERE id=1",
                [],
            )
            .unwrap();
        assert!(output.feed(b"ATVPREP-PROGRESS 99\n", true).is_err());
        assert_eq!(
            conn.raw()
                .query_row("SELECT count(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            before
        );
    }
}

//! Incremental, fenced progress and bounded log events from a running Prepare child.
use super::{executor::FfmpegProgress, logs::LogTail};
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

pub(super) struct ClaimedOutput<'a> {
    pub ctx: Ctx<'a>,
    pub owner: &'a str,
    pub fence: i64,
    pub item: i64,
    pub job: i64,
    parser: FfmpegProgress,
    tail: LogTail,
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
            tail: LogTail::new(8192),
            progress: None,
            flushed: None,
        }
    }
    pub fn feed(&mut self, chunk: &[u8], force: bool) -> Result<()> {
        self.tail.push(chunk);
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
        let _nonblocking = NonblockingWrite::new(self.ctx.conn)?;
        let tx = match rusqlite::Transaction::new_unchecked(
            self.ctx.conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(Error::from)
        {
            Ok(tx) => tx,
            Err(Error::Busy) => {
                self.flushed = Some(Instant::now());
                return Ok(());
            }
            Err(error) => return Err(error),
        };
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
        if !self.tail.is_empty()
            && Jobs(ctx).set_item_log_tail(self.item, self.fence, &self.tail.text())?
        {
            Events(ctx).append(
                self.job,
                "job.log",
                "job_item",
                self.item,
                &serde_json::json!({"tail":self.tail.text()}),
                Some(self.item),
            )?;
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
        output.feed(&vec![b'x'; 50000], true).unwrap();
        let (percent, tail): (i64, String) = conn
            .raw()
            .query_row(
                "SELECT progress,log_tail FROM job_items WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(percent, 75);
        assert!(tail.len() <= 8192);
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

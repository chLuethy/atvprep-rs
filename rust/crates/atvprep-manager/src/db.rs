//! Connections with the manager's policy: WAL, foreign keys on, a busy timeout, and a bounded
//! retry of transactions that still meet a lock.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::{Error, Result};

const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);
const MAX_ATTEMPTS: u32 = 5;
const BASE_DELAY: Duration = Duration::from_millis(50);
const MAX_DELAY: Duration = Duration::from_millis(500);

static MEMORY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
enum Target {
    File(PathBuf),
    /// A named shared-cache in-memory database; `anchor` keeps it alive.
    Memory(String),
}

/// Where the database lives and how its connections behave.
#[derive(Debug, Clone)]
pub struct Db {
    target: Target,
    busy_timeout: Duration,
    anchor: Option<Arc<Mutex<Connection>>>,
}

/// One connection.
pub struct Conn {
    inner: Connection,
    /// The one event id this connection may delete right now (0 = none); read by the
    /// `event_delete_authorized` SQL function that guards the `events` table.
    event_delete: Arc<AtomicI64>,
}

impl Conn {
    /// Open an existing database for inspection without changing its journal mode.
    ///
    /// # Errors
    /// The path is not a regular database file or SQLite cannot open it read-only.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        if std::fs::symlink_metadata(path).map_or(true, |metadata| !metadata.file_type().is_file())
        {
            return Err(Error::Invalid(
                "database must be an existing regular file".to_owned(),
            ));
        }
        let inner = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        inner.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Self {
            inner,
            event_delete: Arc::new(AtomicI64::new(0)),
        })
    }
}

impl Db {
    /// A database file, created when missing.
    ///
    /// # Errors
    ///
    /// The file cannot be opened.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Self {
            target: Target::File(path.to_path_buf()),
            busy_timeout: BUSY_TIMEOUT,
            anchor: None,
        };
        db.conn()?;
        Ok(db)
    }

    /// A private in-memory database that all connections of this `Db` share.
    ///
    /// # Errors
    ///
    /// SQLite cannot create it.
    pub fn memory() -> Result<Self> {
        let name = format!(
            "atvprep-{}-{}",
            std::process::id(),
            MEMORY_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut db = Self {
            target: Target::Memory(name),
            busy_timeout: BUSY_TIMEOUT,
            anchor: None,
        };
        db.anchor = Some(Arc::new(Mutex::new(db.conn()?.inner)));
        Ok(db)
    }

    /// The same database with another busy timeout (tests use a short one).
    #[must_use]
    pub fn with_busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout = timeout;
        self
    }

    /// A new connection with the manager's PRAGMAs applied.
    ///
    /// # Errors
    ///
    /// The database cannot be opened.
    pub fn conn(&self) -> Result<Conn> {
        let inner = match &self.target {
            Target::File(path) => Connection::open(path)?,
            Target::Memory(name) => Connection::open_with_flags(
                format!("file:{name}?mode=memory&cache=shared"),
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_SHARED_CACHE,
            )?,
        };
        inner.busy_timeout(self.busy_timeout)?;
        if matches!(self.target, Target::File(_)) {
            inner.pragma_update(None, "journal_mode", "WAL")?;
        }
        inner.pragma_update(None, "foreign_keys", "ON")?;
        let event_delete = Arc::new(AtomicI64::new(0));
        let allowed = Arc::clone(&event_delete);
        inner.create_scalar_function(
            "event_delete_authorized",
            1,
            FunctionFlags::SQLITE_UTF8,
            move |ctx| {
                let id: i64 = ctx.get(0)?;
                Ok(i64::from(id != 0 && allowed.load(Ordering::SeqCst) == id))
            },
        )?;
        Ok(Conn {
            inner,
            event_delete,
        })
    }
}

fn jitter(limit: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let limit_nanos = u64::try_from(limit.as_nanos()).unwrap_or(u64::MAX).max(1);
    Duration::from_nanos(u64::from(nanos) % limit_nanos)
}

impl Conn {
    /// The raw connection, for repositories that run several statements themselves.
    #[must_use]
    pub const fn raw(&self) -> &Connection {
        &self.inner
    }

    /// Delete one event, the way retention maintenance does: the append-only guard lets exactly
    /// this id through, on this connection, for the duration of the statement.
    ///
    /// # Errors
    ///
    /// The statement fails.
    pub fn delete_event_for_retention(&self, id: i64) -> Result<usize> {
        self.event_delete.store(id, Ordering::SeqCst);
        let outcome = self.inner.execute("DELETE FROM events WHERE id = ?1", [id]);
        self.event_delete.store(0, Ordering::SeqCst);
        Ok(outcome?)
    }

    /// Run `work` in an immediate transaction. A locked database is retried with a jittered,
    /// capped exponential backoff; the last attempt's error is returned.
    ///
    /// # Errors
    ///
    /// Whatever `work` returns, or [`Error::Busy`] after the last attempt.
    pub fn transaction<T>(
        &mut self,
        mut work: impl FnMut(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        retry_busy(|| {
            let tx = self
                .inner
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = work(&tx)?;
            tx.commit()?;
            Ok(value)
        })
    }
}

/// Retry only lock failures after the supplied operation has rolled back its changes.
pub(crate) fn retry_busy<T>(mut work: impl FnMut() -> Result<T>) -> Result<T> {
    for attempt in 0..MAX_ATTEMPTS {
        match work() {
            Err(Error::Busy) if attempt + 1 < MAX_ATTEMPTS => {
                let ceiling = (BASE_DELAY * 2_u32.pow(attempt)).min(MAX_DELAY);
                std::thread::sleep(jitter(ceiling));
            }
            result => return result,
        }
    }
    Err(Error::Busy)
}

/// Begin an immediate transaction with bounded retries before caller work starts.
///
/// # Errors
/// Returns a persistent lock as [`Error::Busy`] and propagates other SQLite failures.
pub fn begin_immediate(conn: &Connection) -> Result<Transaction<'_>> {
    retry_busy(|| {
        Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(Error::from)
    })
}

/// Retry a complete repository write only after its transaction has rolled back.
pub(crate) fn atomic_write<T>(
    ctx: crate::repo::Ctx<'_>,
    mut work: impl FnMut(crate::repo::Ctx<'_>) -> Result<T>,
) -> Result<T> {
    if !ctx.conn.is_autocommit() {
        return work(ctx);
    }
    retry_busy(|| {
        let tx = Transaction::new_unchecked(ctx.conn, TransactionBehavior::Immediate)?;
        let result = work(crate::repo::Ctx {
            conn: &tx,
            clock: ctx.clock,
        })?;
        tx.commit()?;
        Ok(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma(conn: &Conn, name: &str) -> String {
        conn.raw()
            .query_row(&format!("PRAGMA {name}"), [], |row| {
                row.get::<_, rusqlite::types::Value>(0)
            })
            .map(|v| match v {
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Text(t) => t,
                other => format!("{other:?}"),
            })
            .unwrap()
    }

    #[test]
    fn pragmas_are_set_on_every_connection() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("m.db")).unwrap();
        for _ in 0..2 {
            let conn = db.conn().unwrap();
            assert_eq!(pragma(&conn, "journal_mode"), "wal");
            assert_eq!(pragma(&conn, "foreign_keys"), "1");
            assert_eq!(pragma(&conn, "busy_timeout"), "5000");
        }
    }

    #[test]
    fn busy_is_retried_then_reported() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("m.db"))
            .unwrap()
            .with_busy_timeout(Duration::from_millis(10));
        let holder = db.conn().unwrap();
        holder.raw().execute_batch("CREATE TABLE t(x)").unwrap();
        holder.raw().execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut other = db.conn().unwrap();
        let mut attempts = 0;
        let started = std::time::Instant::now();
        let result = other.transaction(|_| {
            attempts += 1;
            Ok(())
        });
        assert!(matches!(result, Err(Error::Busy)), "{result:?}");
        // The closure never ran: BEGIN IMMEDIATE itself met the lock on every attempt.
        assert_eq!(attempts, 0);
        assert!(
            started.elapsed() >= Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
        holder.raw().execute_batch("ROLLBACK").unwrap();
        assert!(other.transaction(|_| Ok(())).is_ok());
    }

    #[test]
    fn memory_databases_share_state_between_connections() {
        let db = Db::memory().unwrap();
        db.conn()
            .unwrap()
            .raw()
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (7)")
            .unwrap();
        let seen: i64 = db
            .conn()
            .unwrap()
            .raw()
            .query_row("SELECT x FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seen, 7);
        let other = Db::memory().unwrap();
        assert!(
            other
                .conn()
                .unwrap()
                .raw()
                .query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))
                .is_err()
        );
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        conn.raw()
            .execute_batch("CREATE TABLE p(id INTEGER PRIMARY KEY); CREATE TABLE c(p INTEGER REFERENCES p(id))")
            .unwrap();
        let result = conn.transaction(|tx| {
            tx.execute("INSERT INTO c VALUES (99)", [])?;
            Ok(())
        });
        assert!(matches!(result, Err(Error::Constraint(_))), "{result:?}");
    }

    #[test]
    fn a_failed_transaction_leaves_nothing_behind() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        conn.raw()
            .execute_batch("CREATE TABLE t(x INTEGER UNIQUE)")
            .unwrap();
        let result: Result<()> = conn.transaction(|tx| {
            tx.execute("INSERT INTO t VALUES (1)", [])?;
            Err(Error::Invalid("stop".to_owned()))
        });
        assert!(matches!(result, Err(Error::Invalid(_))));
        let count: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}

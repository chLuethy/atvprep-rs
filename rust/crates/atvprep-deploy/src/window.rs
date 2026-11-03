//! Local, journaled offline release windows. Failures never reopen the gate.
use crate::{Error, Result};
use atvprep_manager::{
    clock::Clock,
    db::Conn,
    deployment_gate::{DeploymentGate, GateStatus, Operator},
    repo::Ctx,
    schema,
};
use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct WindowOptions {
    pub execute: bool,
    pub services_stopped: bool,
    pub imports_stopped: bool,
    pub timeout: Duration,
    pub backup_directory: PathBuf,
}
pub struct WindowReport {
    pub executed: bool,
    pub generation: i64,
    pub gate_closed: bool,
    pub backup_created: bool,
}

fn refused(step: &str) -> Error {
    Error::Invalid(format!(
        "release window refused at {step}; inspect its journal before retrying"
    ))
}

/// Inspect the current Rust schema and gate without changing database state.
///
/// # Errors
/// Rejects foreign schemas and unavailable or unhealthy gate storage.
pub fn inspect(conn: &Conn, clock: &dyn Clock) -> Result<GateStatus> {
    let version: i64 = conn
        .raw()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| refused("schema inspection"))?;
    if version != schema::VERSION {
        return Err(refused("schema inspection"));
    }
    let mut statement=conn.raw().prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").map_err(|_|refused("schema inspection"))?;
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| refused("schema inspection"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| refused("schema inspection"))?;
    if !tables.iter().map(String::as_str).eq(schema::TABLES) {
        return Err(refused("schema inspection"));
    }
    DeploymentGate(Ctx {
        conn: conn.raw(),
        clock,
    })
    .status()
    .map_err(|_| refused("gate inspection"))
}

fn backup_directory(path: &std::path::Path) -> Result<(PathBuf, std::os::fd::OwnedFd)> {
    let path = path
        .canonicalize()
        .map_err(|_| refused("backup directory"))?;
    let directory = atvprep_core::fd_fix_contract::open_library_root(&path)
        .map_err(|_| refused("backup directory"))?;
    let metadata = rustix::fs::fstat(&directory).map_err(|_| refused("backup directory"))?;
    if metadata.st_uid != rustix::process::geteuid().as_raw() || metadata.st_mode & 0o022 != 0 {
        return Err(refused("backup directory permissions"));
    }
    Ok((path, directory))
}

fn note(journal: &mut std::fs::File, clock: &dyn Clock, step: &str, ok: bool) -> Result<()> {
    serde_json::to_writer(
        &mut *journal,
        &serde_json::json!({"at":clock.now(),"step":step,"ok":ok}),
    )?;
    journal
        .write_all(b"\n")
        .map_err(|_| refused("journal write"))?;
    journal
        .sync_all()
        .map_err(|_| refused("journal synchronization"))
}

fn backup(
    conn: &Conn,
    path: &std::path::Path,
    directory: &std::os::fd::OwnedFd,
    timeout: Duration,
) -> Result<()> {
    let reserved = rustix::fs::openat(
        directory,
        "manager-backup.sqlite.partial",
        rustix::fs::OFlags::RDWR
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .map_err(|_| refused("backup reservation"))?;
    let partial = path.join("manager-backup.sqlite.partial");
    let mut destination = rusqlite::Connection::open_with_flags(
        &partial,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(|_| refused("backup creation"))?;
    {
        let backup = rusqlite::backup::Backup::new(conn.raw(), &mut destination)
            .map_err(|_| refused("backup creation"))?;
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(refused("backup deadline"));
            }
            if backup.step(100).map_err(|_| refused("backup copying"))?
                == rusqlite::backup::StepResult::Done
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let integrity: String = destination
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| refused("backup verification"))?;
    if integrity != "ok" {
        return Err(refused("backup integrity"));
    }
    for table in schema::TABLES {
        let sql = format!("SELECT count(*) FROM {table}");
        let source: i64 = conn
            .raw()
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|_| refused("backup row counts"))?;
        let copied: i64 = destination
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|_| refused("backup row counts"))?;
        if source != copied {
            return Err(refused("backup row counts"));
        }
    }
    drop(destination);
    std::fs::File::from(reserved)
        .sync_all()
        .map_err(|_| refused("backup synchronization"))?;
    rustix::fs::renameat(
        directory,
        "manager-backup.sqlite.partial",
        directory,
        "manager-backup.sqlite",
    )
    .map_err(|_| refused("backup publication"))?;
    Ok(())
}

/// Execute a release window, leaving its gate closed on success and failure.
/// The migration callback must use the staged, authenticated new release.
///
/// # Errors
/// Rejects missing shutdown confirmations, drain timeouts, backup/migration/activation failures.
#[allow(clippy::too_many_lines)] // Journal order is the recovery contract for every release step.
pub fn run(
    conn: &Conn,
    clock: &dyn Clock,
    operator: &Operator,
    options: &WindowOptions,
    migrate: impl FnOnce(i64) -> Result<()>,
    activate: impl FnOnce() -> Result<()>,
) -> Result<WindowReport> {
    operator
        .require_local()
        .map_err(|_| refused("local authorization"))?;
    let database = std::path::Path::new(
        conn.raw()
            .path()
            .ok_or_else(|| refused("database identity"))?,
    );
    Operator::from_local_cli(database).map_err(|_| refused("local authorization"))?;
    let initial = inspect(conn, clock)?;
    let (path, directory) = backup_directory(&options.backup_directory)?;
    if options.timeout.is_zero() || options.timeout > Duration::from_secs(3600) {
        return Err(refused("timeout configuration"));
    }
    if path
        .join("manager-backup.sqlite")
        .symlink_metadata()
        .is_ok()
        || path.join("window.jsonl").symlink_metadata().is_ok()
    {
        return Err(refused(
            "existing backup or journal; choose a fresh backup directory",
        ));
    }
    if !options.execute {
        return Ok(WindowReport {
            executed: false,
            generation: initial.generation,
            gate_closed: initial.mode == "maintenance",
            backup_created: false,
        });
    }
    if !options.services_stopped || !options.imports_stopped {
        return Err(refused("stopped-services confirmation"));
    }
    let _lock = crate::local::window_lock(database)?;
    let journal = rustix::fs::openat(
        &directory,
        "window.jsonl",
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .map_err(|_| refused("journal reservation"))?;
    let mut journal = std::fs::File::from(journal);
    note(&mut journal, clock, "preflight", true)?;
    let gate = DeploymentGate(Ctx {
        conn: conn.raw(),
        clock,
    });
    let mut step = "close";
    let result = (|| {
        let mut status = gate
            .close(operator, "release window")
            .map_err(|_| refused("gate close"))?;
        note(&mut journal, clock, "close", true)?;
        step = "drain";
        let deadline = Instant::now() + options.timeout;
        while !status.quiesced {
            if Instant::now() >= deadline {
                return Err(Error::DrainTimeout);
            }
            std::thread::sleep(Duration::from_millis(100));
            status = gate.status().map_err(|_| refused("gate drain"))?;
            if !status.healthy || status.mode != "maintenance" {
                return Err(refused("gate drain"));
            }
        }
        note(&mut journal, clock, "drain", true)?;
        step = "backup";
        conn.raw()
            .execute_batch("BEGIN EXCLUSIVE; ROLLBACK")
            .map_err(|_| refused("active database writer"))?;
        backup(conn, &path, &directory, options.timeout)?;
        note(&mut journal, clock, "backup", true)?;
        step = "begin-migration";
        status = gate
            .begin_offline_migration(operator, status.generation)
            .map_err(|_| refused("begin migration"))?;
        note(&mut journal, clock, step, true)?;
        step = "migrate";
        migrate(status.generation)?;
        note(&mut journal, clock, step, true)?;
        step = "activate";
        activate()?;
        note(&mut journal, clock, step, true)?;
        step = "finish";
        status = gate
            .finish_offline_migration(operator, status.generation)
            .map_err(|_| refused("finish migration"))?;
        note(&mut journal, clock, step, true)?;
        Ok(WindowReport {
            executed: true,
            generation: status.generation,
            gate_closed: status.mode == "maintenance",
            backup_created: true,
        })
    })();
    if result.is_err() {
        let _ = note(&mut journal, clock, step, false);
    }
    result
}

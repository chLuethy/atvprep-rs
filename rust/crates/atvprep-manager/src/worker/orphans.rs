//! Start-up sweep for working files that a crash left beside their source.
//!
//! A file goes only when its name matches the working-file pattern exactly, it is old enough, no
//! claimed or running item works in its directory and the PID in its name is gone. Where the PID
//! cannot be checked, the age gate alone decides.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use atvprep_core::prepare::paths::parse_temp_name;

use crate::Result;
use crate::clock::format_micros;
use crate::deployment_gate::DeploymentGate;
use crate::repo::Ctx;

/// A working file younger than this may still belong to a run that is just ending.
const MIN_AGE: Duration = Duration::from_mins(10);
const RETENTION_MICROS: i64 = 7 * 86_400 * 1_000_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrphanSweep {
    /// The sweep looked at the libraries; `false` while the gate is closed.
    pub ran: bool,
    pub removed: usize,
}

/// Delete orphaned partial rewrites under the enabled library roots and record each one.
///
/// Call after recovery has marked expired items `interrupted`. Nothing is touched while the gate
/// is closed or in migration.
///
/// # Errors
///
/// SQLite fails.
pub fn sweep(ctx: Ctx<'_>) -> Result<OrphanSweep> {
    let gate = DeploymentGate(ctx).status()?;
    if !gate.healthy || gate.mode != "open" || gate.phase != "steady" {
        return Ok(OrphanSweep::default());
    }
    let roots = roots(ctx)?;
    let busy = busy_directories(ctx, &roots)?;
    let now =
        UNIX_EPOCH + Duration::from_micros(u64::try_from(ctx.clock.now_micros()).unwrap_or(0));
    let mut removed = 0;
    for (root_id, root) in &roots {
        let mut directories = vec![root.clone()];
        while let Some(directory) = directories.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            let busy_here = busy.contains(&directory);
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    directories.push(entry.path());
                } else if kind.is_file()
                    && !busy_here
                    && is_orphan(&entry.path(), &entry.file_name().to_string_lossy(), now)
                    && std::fs::remove_file(entry.path()).is_ok()
                {
                    removed += 1;
                    record(ctx, *root_id, root, &entry.path())?;
                }
            }
        }
    }
    Ok(OrphanSweep { ran: true, removed })
}

fn roots(ctx: Ctx<'_>) -> Result<Vec<(i64, PathBuf)>> {
    let mut statement = ctx
        .conn
        .prepare("SELECT id, canonical_path FROM library_roots WHERE enabled=1 ORDER BY id")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?)))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The directories in which a claimed or running item works right now.
fn busy_directories(ctx: Ctx<'_>, roots: &[(i64, PathBuf)]) -> Result<HashSet<PathBuf>> {
    let mut statement = ctx.conn.prepare(
        "SELECT DISTINCT m.root_id, m.relative_path FROM job_items i
         JOIN media_files m ON m.id = i.media_file_id
         WHERE i.status IN ('claimed','running')",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut busy = HashSet::new();
    for row in rows {
        let (root_id, relative) = row?;
        if let Some((_, root)) = roots.iter().find(|(id, _)| *id == root_id)
            && let Some(parent) = root.join(relative).parent()
        {
            busy.insert(parent.to_path_buf());
        }
    }
    Ok(busy)
}

fn is_orphan(path: &Path, name: &str, now: SystemTime) -> bool {
    let Some(pid) = parse_temp_name(name) else {
        return false;
    };
    let old_enough = std::fs::symlink_metadata(path)
        .and_then(|meta| meta.modified())
        .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age >= MIN_AGE));
    old_enough && !pid_alive(pid)
}

/// Whether `pid` exists in this PID namespace. A process of another user still counts as alive.
/// When the PID is not a valid process id the check is impossible and only the age gate applies.
fn pid_alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    !matches!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    )
}

fn record(ctx: Ctx<'_>, root_id: i64, root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    let now = ctx.clock.now_micros();
    ctx.conn.execute(
        "INSERT INTO events(topic, entity_type, entity_id, payload_json, created_at, expires_at)
         VALUES ('orphan.removed', 'library_root', ?1, ?2, ?3, ?4)",
        rusqlite::params![
            root_id,
            serde_json::json!({ "root_id": root_id, "relative_path": relative }).to_string(),
            format_micros(now),
            format_micros(now + RETENTION_MICROS),
        ],
    )?;
    Ok(())
}

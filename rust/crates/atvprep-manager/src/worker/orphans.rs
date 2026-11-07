//! Start-up sweep for working files that a crash left beside their source.
//!
//! A file goes only when its name matches the working-file pattern exactly, it is old enough, no
//! claimed or running item works in its directory and the PID in its name is gone. Where the PID
//! cannot be checked, the age gate alone decides.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use atvprep_core::prepare::paths::parse_temp_name;

use rusqlite::OptionalExtension;

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
                    record(ctx, *root_id, root, &entry.path(), None)?;
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

fn record(
    ctx: Ctx<'_>,
    root_id: i64,
    root: &Path,
    path: &Path,
    manual_size: Option<u64>,
) -> Result<()> {
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    let payload = match manual_size {
        Some(size) => serde_json::json!({
            "root_id": root_id, "relative_path": relative, "size": size, "trigger": "manual"
        }),
        None => serde_json::json!({ "root_id": root_id, "relative_path": relative }),
    };
    insert_event(ctx, "orphan.removed", root_id, &payload)
}

fn insert_event(
    ctx: Ctx<'_>,
    topic: &str,
    root_id: i64,
    payload: &serde_json::Value,
) -> Result<()> {
    let now = ctx.clock.now_micros();
    ctx.conn.execute(
        "INSERT INTO events(topic, entity_type, entity_id, payload_json, created_at, expires_at)
         VALUES (?1, 'library_root', ?2, ?3, ?4, ?5)",
        rusqlite::params![
            topic,
            root_id,
            payload.to_string(),
            format_micros(now),
            format_micros(now + RETENTION_MICROS),
        ],
    )?;
    Ok(())
}

/// How many entries of the list a scan report keeps; count and bytes always cover all of them.
const REPORT_LIST_MAX: usize = 20;

/// A working file that matches the exact pattern, whatever its age or the state of its PID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub relative_path: String,
    pub size: u64,
    /// Modification time in microseconds since the Unix epoch.
    pub modified_micros: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed {
        count: usize,
        bytes: u64,
    },
    /// The files differ from what the operator confirmed; nothing was deleted.
    Changed {
        count: usize,
        bytes: u64,
    },
}

fn root_path(ctx: Ctx<'_>, root_id: i64) -> Result<Option<PathBuf>> {
    Ok(ctx
        .conn
        .query_row(
            "SELECT canonical_path FROM library_roots WHERE id=?1",
            [root_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(PathBuf::from))
}

/// Every file of one library that has the working-file name, except in directories where a
/// claimed or running item works. Symbolic links are neither listed nor followed.
///
/// # Errors
///
/// SQLite fails.
pub fn find(ctx: Ctx<'_>, root_id: i64) -> Result<Vec<Orphan>> {
    let Some(root) = root_path(ctx, root_id)? else {
        return Ok(Vec::new());
    };
    let busy = busy_directories(ctx, &[(root_id, root.clone())])?;
    let mut found = Vec::new();
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
                && parse_temp_name(&entry.file_name().to_string_lossy()).is_some()
                && let Ok(meta) = entry.metadata()
            {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(UNIX_EPOCH).ok());
                found.push(Orphan {
                    relative_path: entry
                        .path()
                        .strip_prefix(&root)
                        .unwrap_or(&entry.path())
                        .to_string_lossy()
                        .into_owned(),
                    size: meta.len(),
                    modified_micros: modified
                        .and_then(|d| i64::try_from(d.as_micros()).ok())
                        .unwrap_or(0),
                });
            }
        }
    }
    found.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(found)
}

/// Store the leftovers of a library on a finished scan. No leftovers store nothing.
///
/// # Errors
///
/// SQLite fails.
pub fn report_for_scan(ctx: Ctx<'_>, scan_id: i64, root_id: i64) -> Result<()> {
    let found = find(ctx, root_id)?;
    let report = (!found.is_empty()).then(|| {
        serde_json::json!({
            "count": found.len(),
            "bytes": found.iter().map(|o| o.size).sum::<u64>(),
            "files": found.iter().take(REPORT_LIST_MAX).map(|o| serde_json::json!({
                "relative_path": o.relative_path,
                "size": o.size,
                "modified_at": format_micros(o.modified_micros),
            })).collect::<Vec<_>>(),
        })
        .to_string()
    });
    ctx.conn.execute(
        "UPDATE scan_runs SET orphans_json=?1 WHERE id=?2",
        rusqlite::params![report, scan_id],
    )?;
    Ok(())
}

fn refresh_latest_report(ctx: Ctx<'_>, root_id: i64) -> Result<()> {
    let latest: Option<i64> = ctx
        .conn
        .query_row(
            "SELECT id FROM scan_runs WHERE root_id=?1 AND status='succeeded'
             ORDER BY generation DESC, id DESC LIMIT 1",
            [root_id],
            |row| row.get(0),
        )
        .optional()?;
    match latest {
        Some(scan_id) => report_for_scan(ctx, scan_id, root_id),
        None => Ok(()),
    }
}

/// Delete the leftovers of one library after the operator confirmed `confirmed_count` files of
/// `confirmed_bytes` together. When the files found now differ, nothing is deleted and the fresh
/// numbers return. Only files with the exact working-file name inside the library go, and none in
/// a directory where a claimed or running item works. Each deletion is recorded, plus one summary.
///
/// # Errors
///
/// SQLite fails.
pub fn remove(
    ctx: Ctx<'_>,
    root_id: i64,
    confirmed_count: usize,
    confirmed_bytes: u64,
) -> Result<RemoveOutcome> {
    let found = find(ctx, root_id)?;
    let bytes: u64 = found.iter().map(|o| o.size).sum();
    if found.len() != confirmed_count || bytes != confirmed_bytes {
        refresh_latest_report(ctx, root_id)?;
        return Ok(RemoveOutcome::Changed {
            count: found.len(),
            bytes,
        });
    }
    let Some(root) = root_path(ctx, root_id)? else {
        return Ok(RemoveOutcome::Removed { count: 0, bytes: 0 });
    };
    let (mut count, mut removed_bytes) = (0, 0);
    for orphan in &found {
        let path = root.join(&orphan.relative_path);
        // A link swapped in since `find` must not be followed.
        let is_file = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
        if is_file && std::fs::remove_file(&path).is_ok() {
            count += 1;
            removed_bytes += orphan.size;
            record(ctx, root_id, &root, &path, Some(orphan.size))?;
        }
    }
    if count > 0 {
        insert_event(
            ctx,
            "orphan.cleanup",
            root_id,
            &serde_json::json!({ "root_id": root_id, "count": count, "bytes": removed_bytes }),
        )?;
    }
    refresh_latest_report(ctx, root_id)?;
    Ok(RemoveOutcome::Removed {
        count,
        bytes: removed_bytes,
    })
}

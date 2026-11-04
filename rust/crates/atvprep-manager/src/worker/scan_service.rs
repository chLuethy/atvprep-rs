//! Atomic persistence of scheduler-owned, descriptor-observed scans.
use super::scanner::{SeenFile, observe, visit_checked};
use crate::{
    Error, Result, deployment_gate::DeploymentGate, media::Media, repo::Ctx, scans::Scans,
    scheduler::SchedulerLease, sidecars::Sidecars,
};
use std::{
    io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct ScanOptions {
    pub follow_mounts: bool,
    pub excluded: Vec<String>,
    pub settle_seconds: i64,
    pub lease_seconds: i64,
    pub shutdown: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
pub struct ScanResult {
    pub seen: usize,
    /// Bounded preview; the complete settled set remains in the database.
    pub settled_media: Vec<i64>,
}

/// Collect and transactionally persist one scheduler-owned scan.
///
/// # Errors
/// Rejects stale ownership, root drift, unsafe inputs and database failures.
#[allow(clippy::too_many_lines)] // Keep collection and commit bound to the same root and fence.
pub fn run(
    ctx: Ctx<'_>,
    scan_id: i64,
    owner: &str,
    fence: i64,
    options: &ScanOptions,
) -> Result<ScanResult> {
    let scan = Scans(ctx)
        .get(scan_id)?
        .ok_or_else(|| Error::Invalid("scan not found".to_owned()))?;
    if scan.claimed_by.as_deref() != Some(owner)
        || scan.fencing_token != fence
        || scan.status != "running"
        || options.lease_seconds < 3
    {
        return Err(Error::Conflict(
            "scan is not held by this worker".to_owned(),
        ));
    }
    let root: String = ctx.conn.query_row(
        "SELECT canonical_path FROM library_roots WHERE id=?1",
        [scan.root_id],
        |row| row.get(0),
    )?;
    let root = Path::new(&root);
    let original = atvprep_core::fd_fix_contract::open_library_root(root)
        .map_err(|_| Error::Invalid("scan root is unavailable".to_owned()))?;
    let root_stat = rustix::fs::fstat(&original)
        .map_err(|_| Error::Invalid("cannot inspect scan root".to_owned()))?;
    let lease = SchedulerLease::new(ctx);
    let mut last = Instant::now();
    let mut check = || {
        if options
            .shutdown
            .as_ref()
            .is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Acquire))
        {
            return Err(Error::Invalid("scan interrupted by shutdown".to_owned()));
        }
        if last.elapsed() >= Duration::from_secs(1) {
            if !lease.heartbeat(owner, Some(fence), options.lease_seconds)? {
                return Err(Error::Conflict("scan lost its scheduler lease".to_owned()));
            }
            last = Instant::now();
        }
        if !Scans(ctx).is_authoritative(scan_id, scan.root_id)? {
            return Err(Error::Conflict("scan lost its authority".to_owned()));
        }
        Ok(())
    };
    // A scan that stops before it starts would otherwise stay "running" and count as active work.
    if let Err(error) = check() {
        let _ = Scans(ctx).abandon(scan.id);
        return Err(error);
    }
    let gate = DeploymentGate(ctx);
    let Some(permit) = gate.acquire_permit("scan", owner)? else {
        let _ = Scans(ctx).abandon(scan.id);
        return Err(Error::Conflict(
            "maintenance: scans are not accepted".to_owned(),
        ));
    };
    let result = (|| {
        let file = tempfile::tempfile()
            .map_err(|_| Error::Invalid("cannot buffer scan observations".to_owned()))?;
        let mut buffer = BufWriter::new(file);
        let mut seen = 0;
        visit_checked(
            root,
            options.follow_mounts,
            &options.excluded,
            &mut check,
            |file| {
                let observation = observe(
                    root,
                    &file.relative_path,
                    options.settle_seconds,
                    ctx.clock.now_micros(),
                )?;
                serde_json::to_writer(&mut buffer, &observation)
                    .map_err(|_| Error::Invalid("cannot buffer scan observations".to_owned()))?;
                buffer
                    .write_all(b"\n")
                    .map_err(|_| Error::Invalid("cannot buffer scan observations".to_owned()))?;
                seen += 1;
                Ok(())
            },
        )?;
        let mut buffer = buffer
            .into_inner()
            .map_err(|_| Error::Invalid("cannot finish scan observations".to_owned()))?;
        crate::db::retry_busy(|| {
            buffer
                .seek(SeekFrom::Start(0))
                .map_err(|_| Error::Invalid("cannot rewind scan observations".to_owned()))?;
            let tx = rusqlite::Transaction::new_unchecked(
                ctx.conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let write = Ctx {
                conn: &tx,
                clock: ctx.clock,
            };
            let current = atvprep_core::fd_fix_contract::open_library_root(root)
                .map_err(|_| Error::Invalid("scan root changed".to_owned()))?;
            let current = rustix::fs::fstat(&current)
                .map_err(|_| Error::Invalid("cannot inspect scan root".to_owned()))?;
            if (current.st_dev, current.st_ino) != (root_stat.st_dev, root_stat.st_ino) {
                return Err(Error::Invalid("scan root changed".to_owned()));
            }
            let mut settled = Vec::new();
            for line in BufReader::new(&mut buffer).lines() {
                check()?;
                let line =
                    line.map_err(|_| Error::Invalid("cannot read scan observations".to_owned()))?;
                let file: SeenFile = serde_json::from_str(&line)
                    .map_err(|_| Error::Invalid("invalid buffered scan observation".to_owned()))?;
                let media = Media(write)
                    .upsert(
                        scan.root_id,
                        &file.relative_path,
                        &file.meta,
                        Some((scan.id, scan.generation)),
                    )?
                    .ok_or_else(|| Error::Conflict("scan lost its write authority".to_owned()))?;
                for sidecar in &file.sidecars {
                    Sidecars(write)
                        .upsert(
                            media.id,
                            &sidecar.relative_path,
                            &super::scanner::fingerprint(sidecar.size, sidecar.mtime_ns),
                            sidecar.size,
                            sidecar.mtime_ns,
                            Some(scan.id),
                        )?
                        .ok_or_else(|| {
                            Error::Conflict("scan lost its sidecar authority".to_owned())
                        })?;
                }
                let names: Vec<_> = file
                    .sidecars
                    .iter()
                    .map(|sidecar| &sidecar.relative_path)
                    .collect();
                let sql = format!(
                    "DELETE FROM sidecars WHERE media_file_id=:media AND relative_path NOT IN (SELECT value FROM json_each(:names)) AND EXISTS(SELECT 1 FROM scan_runs AS scan WHERE {})",
                    crate::scans::CLAIMED_SCAN_AUTHORITY
                );
                write.conn.execute(&sql,rusqlite::named_params!{":media":media.id,":names":serde_json::to_string(&names).map_err(|_|Error::Invalid("cannot encode sidecar set".to_owned()))?,":scan_id":scan.id,":root_id":scan.root_id,":now":ctx.clock.now()})?;
                if !file.unsettled && settled.len() < 32 {
                    settled.push(media.id);
                }
            }
            if Scans(write).finish(scan.id, "succeeded", true)?.status != "succeeded" {
                return Err(Error::Conflict(
                    "scan lost its completion authority".to_owned(),
                ));
            }
            tx.commit()?;
            Ok(ScanResult {
                seen,
                settled_media: settled,
            })
        })
    })();
    if result.is_err()
        && Scans(ctx)
            .is_authoritative(scan.id, scan.root_id)
            .unwrap_or(false)
    {
        let status = if options
            .shutdown
            .as_ref()
            .is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Acquire))
        {
            "interrupted"
        } else {
            "failed"
        };
        let _ = crate::db::retry_busy(|| Scans(ctx).finish(scan.id, status, false));
    } else if result.is_err() {
        // Superseded by a newer scan or cut off from its lease: its own finish would be a no-op.
        let _ = crate::db::retry_busy(|| Scans(ctx).abandon(scan.id));
    }
    let release = gate.release_permit(&permit);
    match result {
        Ok(result) => {
            release?;
            Ok(result)
        }
        Err(error) => {
            let _ = release;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{clock::SystemClock, db::Db, library::LibraryRoots, scheduler::Scheduler, schema};

    fn pass(ctx: Ctx<'_>, scheduler: &Scheduler<'_>, root: i64) -> ScanResult {
        Scans(ctx).request_manual(root).unwrap();
        let scan = scheduler.claim_manual_scan().unwrap().unwrap();
        run(
            ctx,
            scan.id,
            "worker",
            scheduler.lease.fencing_token.unwrap(),
            &ScanOptions {
                follow_mounts: false,
                excluded: vec![],
                settle_seconds: 0,
                lease_seconds: 30,
                shutdown: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn large_scan_persists_every_file_with_a_bounded_result_preview() {
        let directory = tempfile::tempdir().unwrap();
        let root_path = directory.path().canonicalize().unwrap();
        for index in 0..128 {
            std::fs::write(root_path.join(format!("film-{index}.mp4")), b"media").unwrap();
        }
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = SystemClock;
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let root = LibraryRoots(ctx)
            .create("Movies", root_path.to_str().unwrap())
            .unwrap();
        let mut scheduler = Scheduler::new(ctx, "worker", 30);
        assert!(scheduler.acquire(3600).unwrap());
        let report = pass(ctx, &scheduler, root.id);
        assert_eq!(report.seen, 128);
        assert_eq!(report.settled_media.len(), 32);
        let count: i64 = ctx
            .conn
            .query_row("SELECT count(*) FROM media_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 128);
    }

    #[test]
    fn complete_scans_persist_sidecars_and_reconcile_missing_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let root_path = directory.path().canonicalize().unwrap();
        std::fs::write(root_path.join("film.mp4"), b"media").unwrap();
        std::fs::write(root_path.join("film.en.srt"), b"subtitle").unwrap();
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = SystemClock;
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let root = LibraryRoots(ctx)
            .create("Movies", root_path.to_str().unwrap())
            .unwrap();
        let mut scheduler = Scheduler::new(ctx, "worker", 30);
        assert!(scheduler.acquire(3600).unwrap());
        let report = pass(ctx, &scheduler, root.id);
        assert_eq!(report.seen, 1);
        assert_eq!(report.settled_media.len(), 1);
        let media = Media(ctx)
            .get_by_path(root.id, "film.mp4")
            .unwrap()
            .unwrap();
        assert_eq!(media.size, 5);
        assert_eq!(Sidecars(ctx).list_for_media(media.id).unwrap().len(), 1);
        std::fs::remove_file(root_path.join("film.en.srt")).unwrap();
        pass(ctx, &scheduler, root.id);
        assert!(Sidecars(ctx).list_for_media(media.id).unwrap().is_empty());
        std::fs::remove_file(root_path.join("film.mp4")).unwrap();
        let report = pass(ctx, &scheduler, root.id);
        assert_eq!(report.seen, 0);
        assert_eq!(
            Media(ctx).get(media.id).unwrap().unwrap().discovery_state,
            "missing"
        );
        let permits: i64 = ctx
            .conn
            .query_row(
                "SELECT count(*) FROM deployment_permits WHERE released_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(permits, 0);
    }
}

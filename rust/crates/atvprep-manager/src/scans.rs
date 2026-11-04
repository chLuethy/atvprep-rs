//! Scan runs and the authority rules that keep a stale scan from changing anything.

use rusqlite::{OptionalExtension, named_params};

use crate::models::ScanRun;
use crate::repo::Ctx;
use crate::{Error, Result};

/// The scan `:scan_id` of root `:root_id` is a claimed scan, the newest of its root, and
/// its scheduler lease is still held with the same fencing token at `:now`.
pub(crate) const CLAIMED_SCAN_AUTHORITY: &str = "
    scan.id=:scan_id AND scan.root_id=:root_id
    AND scan.claimed_by IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM scan_runs AS newer
        WHERE newer.root_id=scan.root_id
          AND (newer.generation > scan.generation
            OR (newer.generation=scan.generation AND newer.id > scan.id)))
    AND EXISTS (SELECT 1 FROM scheduler_leases AS lease
        WHERE lease.id=1 AND lease.owner_id=scan.claimed_by
          AND lease.fencing_token=scan.fencing_token
          AND lease.lease_expires_at > :now)";

pub struct Scans<'a>(pub Ctx<'a>);

impl Scans<'_> {
    /// # Errors
    ///
    /// SQLite fails.
    pub fn get(&self, scan_id: i64) -> Result<Option<ScanRun>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT * FROM scan_runs WHERE id = ?1",
                [scan_id],
                ScanRun::from_row,
            )
            .optional()?)
    }

    fn require(&self, scan_id: i64) -> Result<ScanRun> {
        self.get(scan_id)?
            .ok_or_else(|| Error::Invalid(format!("unknown scan id: {scan_id}")))
    }

    /// # Errors
    ///
    /// An invalid trigger, or the generation is taken.
    pub fn start(&self, root_id: i64, generation: i64, trigger: &str) -> Result<ScanRun> {
        if !matches!(trigger, "scheduled" | "manual") {
            return Err(Error::Invalid("invalid scan trigger".to_owned()));
        }
        self.0.conn.execute(
            "INSERT INTO scan_runs(root_id, generation, trigger, status, started_at)
             VALUES (?1, ?2, ?3, 'running', ?4)",
            rusqlite::params![root_id, generation, trigger, self.0.clock.now()],
        )?;
        self.require(self.0.conn.last_insert_rowid())
    }

    /// Durably enqueue one operator-requested scan. A repeated request while one still waits shares
    /// it, so a double click queues one scan.
    ///
    /// # Errors
    ///
    /// The root is unknown.
    pub fn request_manual(&self, root_id: i64) -> Result<ScanRun> {
        let known: Option<i64> = self
            .0
            .conn
            .query_row(
                "SELECT id FROM library_roots WHERE id = ?1",
                [root_id],
                |r| r.get(0),
            )
            .optional()?;
        if known.is_none() {
            return Err(Error::Invalid(format!("unknown root id: {root_id}")));
        }
        let waiting: Option<i64> = self
            .0
            .conn
            .query_row(
                "SELECT id FROM scan_runs WHERE root_id=?1 AND trigger='manual' AND status='queued'
                 ORDER BY id LIMIT 1",
                [root_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = waiting {
            return self.require(id);
        }
        let generation: Option<i64> = self.0.conn.query_row(
            "SELECT max(generation) FROM scan_runs WHERE root_id = ?1",
            [root_id],
            |r| r.get(0),
        )?;
        self.0.conn.execute(
            "INSERT INTO scan_runs(root_id, generation, trigger, status, started_at)
             VALUES (?1, ?2, 'manual', 'queued', ?3)",
            rusqlite::params![root_id, generation.unwrap_or(0) + 1, self.0.clock.now()],
        )?;
        self.require(self.0.conn.last_insert_rowid())
    }

    /// End a scan run that can no longer finish normally (superseded, lease lost, refused) as
    /// `interrupted`, whatever its authority. A no-op once it has ended.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn abandon(&self, scan_id: i64) -> Result<()> {
        self.0.conn.execute(
            "UPDATE scan_runs SET status='interrupted', finished_at=?1
             WHERE id=?2 AND status IN ('queued','claimed','running')",
            rusqlite::params![self.0.clock.now(), scan_id],
        )?;
        Ok(())
    }

    /// Whether `scan_id` is the newest scan of the root and, for a claimed scan, still
    /// holds its lease.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn is_authoritative(&self, scan_id: i64, root_id: i64) -> Result<bool> {
        let Some(scan) = self.get(scan_id)? else {
            return Ok(false);
        };
        if scan.root_id != root_id {
            return Ok(false);
        }
        let latest: Option<i64> = self
            .0
            .conn
            .query_row(
                "SELECT id FROM scan_runs WHERE root_id=?1 ORDER BY generation DESC, id DESC LIMIT 1",
                [root_id],
                |r| r.get(0),
            )
            .optional()?;
        if latest != Some(scan_id) {
            return Ok(false);
        }
        if scan.claimed_by.is_none() {
            return Ok(true);
        }
        let held: Option<i64> = self
            .0
            .conn
            .query_row(
                "SELECT id FROM scheduler_leases WHERE id=1 AND owner_id=?1 AND fencing_token=?2
                 AND lease_expires_at > ?3",
                rusqlite::params![scan.claimed_by, scan.fencing_token, self.0.clock.now()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(held.is_some())
    }

    /// Finish a scan. `status` accepts the legacy `completed`/`aborted`. A scan that is no longer
    /// authoritative is left as it is. A succeeded scan marks the files it did not see as missing.
    ///
    /// # Errors
    ///
    /// Unknown scan or invalid status.
    pub fn finish(&self, scan_id: i64, status: &str, reconcile_missing: bool) -> Result<ScanRun> {
        let scan = self.require(scan_id)?;
        let normalized = match status {
            "completed" => "succeeded",
            "aborted" => "interrupted",
            other => other,
        };
        if !matches!(
            normalized,
            "succeeded" | "failed" | "cancelled" | "interrupted"
        ) {
            return Err(Error::Invalid("invalid scan status".to_owned()));
        }
        let now = self.0.clock.now();
        let changed = self.0.conn.execute(
            "UPDATE scan_runs SET status=:status, finished_at=:finished_at
             WHERE id=:scan_id AND root_id=:root_id
               AND NOT EXISTS (SELECT 1 FROM scan_runs AS newer
                   WHERE newer.root_id=scan_runs.root_id
                     AND (newer.generation > scan_runs.generation
                       OR (newer.generation=scan_runs.generation AND newer.id > scan_runs.id)))
               AND (claimed_by IS NULL OR EXISTS
                   (SELECT 1 FROM scheduler_leases
                    WHERE id=1 AND owner_id=scan_runs.claimed_by
                      AND fencing_token=scan_runs.fencing_token
                      AND lease_expires_at > :now))",
            named_params! {
                ":status": normalized, ":finished_at": now, ":scan_id": scan_id,
                ":root_id": scan.root_id, ":now": now,
            },
        )?;
        if changed == 0 {
            return Ok(scan);
        }
        if reconcile_missing && normalized == "succeeded" {
            self.0.conn.execute(
                "UPDATE media_files
                 SET discovery_state='missing',
                     dismissed_at=CASE WHEN discovery_state='missing' THEN dismissed_at END
                 WHERE root_id=:root_id
                   AND (last_seen_scan_id IS NULL OR last_seen_scan_id != :scan_id)
                   AND EXISTS (SELECT 1 FROM scan_runs AS authoritative
                       WHERE authoritative.id=:scan_id AND authoritative.root_id=:root_id
                         AND authoritative.status=:status
                         AND NOT EXISTS (SELECT 1 FROM scan_runs AS newer
                             WHERE newer.root_id=authoritative.root_id
                               AND (newer.generation > authoritative.generation
                                 OR (newer.generation=authoritative.generation
                                     AND newer.id > authoritative.id)))
                         AND (authoritative.claimed_by IS NULL OR EXISTS
                             (SELECT 1 FROM scheduler_leases
                              WHERE id=1 AND owner_id=authoritative.claimed_by
                                AND fencing_token=authoritative.fencing_token
                                AND lease_expires_at > :now)))",
                named_params! {
                    ":scan_id": scan_id, ":root_id": scan.root_id, ":status": normalized,
                    ":now": self.0.clock.now(),
                },
            )?;
        }
        self.require(scan_id)
    }

    /// Record one file a scan saw. `None` when the scan is stale or lost its lease.
    ///
    /// # Errors
    ///
    /// Unknown scan, a root that is not the scan's, or an incomplete scan identity.
    pub fn record_seen(
        &self,
        scan_id: i64,
        root_id: i64,
        relative_path: &str,
        meta: &crate::models::MediaMeta,
    ) -> Result<Option<crate::models::MediaFile>> {
        let scan = self.require(scan_id)?;
        if scan.root_id != root_id {
            return Err(Error::Invalid(format!(
                "scan {scan_id} does not belong to root {root_id}"
            )));
        }
        let fenced = scan.claimed_by.is_some();
        if !fenced && !self.is_authoritative(scan_id, root_id)? {
            return Ok(None);
        }
        crate::media::Media(self.0).upsert(
            root_id,
            relative_path,
            meta,
            Some((scan_id, scan.generation)),
        )
    }
}

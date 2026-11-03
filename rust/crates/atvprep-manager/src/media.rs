//! Media files, and the scan-authority guarded upsert.

use rusqlite::{OptionalExtension, named_params};

use crate::models::{MediaFile, MediaMeta};
use crate::repo::Ctx;
use crate::scans::CLAIMED_SCAN_AUTHORITY;
use crate::{Error, Result};

pub struct Media<'a>(pub Ctx<'a>);

impl Media<'_> {
    /// # Errors
    ///
    /// SQLite fails.
    pub fn get(&self, media_id: i64) -> Result<Option<MediaFile>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT * FROM media_files WHERE id = ?1",
                [media_id],
                MediaFile::from_row,
            )
            .optional()?)
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn get_by_path(&self, root_id: i64, relative_path: &str) -> Result<Option<MediaFile>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT * FROM media_files WHERE root_id = ?1 AND relative_path = ?2",
                rusqlite::params![root_id, relative_path],
                MediaFile::from_row,
            )
            .optional()?)
    }

    /// Hide every file that is missing from the library. Rows, jobs and inspections are kept.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn dismiss_missing(&self) -> Result<usize> {
        Ok(self.0.conn.execute(
            "UPDATE media_files SET dismissed_at = ?1
             WHERE discovery_state = 'missing' AND dismissed_at IS NULL",
            [self.0.clock.now()],
        )?)
    }

    /// A new, present, not yet inspected file.
    ///
    /// # Errors
    ///
    /// An incomplete scan identity, or the path exists in the root.
    pub fn create(&self, root_id: i64, relative_path: &str, meta: &MediaMeta) -> Result<MediaFile> {
        meta.require_identity()?;
        self.insert(root_id, relative_path, meta, None)?;
        self.get_by_path(root_id, relative_path)?
            .ok_or_else(|| Error::NotFound(relative_path.to_owned()))
    }

    fn insert(
        &self,
        root_id: i64,
        relative_path: &str,
        meta: &MediaMeta,
        scan: Option<(i64, i64)>,
    ) -> Result<()> {
        self.0.conn.execute(
            "INSERT INTO media_files
             (root_id, relative_path, kind, suffix, size, mtime_ns, fingerprint,
              scanned_device, scanned_inode, scanned_ctime_ns, sidecar_fingerprint,
              discovery_state, compliance_state, workflow_state, desired_action, last_seen_at,
              last_seen_scan_id, last_seen_generation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'unknown', 'idle', 'none',
                     ?13, ?14, ?15)",
            rusqlite::params![
                root_id,
                relative_path,
                meta.kind,
                meta.suffix,
                meta.size,
                meta.mtime_ns,
                meta.fingerprint,
                meta.scanned_device,
                meta.scanned_inode,
                meta.scanned_ctime_ns,
                meta.sidecar_fingerprint,
                meta.state(),
                self.0.clock.now(),
                scan.map(|s| s.0),
                scan.map(|s| s.1),
            ],
        )?;
        Ok(())
    }

    /// Insert or update the file a scan saw. With `scan = Some((id, generation))` the write only
    /// happens while that scan is authoritative; otherwise `None` is returned and nothing changes.
    ///
    /// # Errors
    ///
    /// An incomplete scan identity, or SQLite fails.
    pub fn upsert(
        &self,
        root_id: i64,
        relative_path: &str,
        meta: &MediaMeta,
        scan: Option<(i64, i64)>,
    ) -> Result<Option<MediaFile>> {
        meta.require_identity()?;
        if let Some((scan_id, generation)) = scan {
            let run = crate::scans::Scans(self.0).get(scan_id)?;
            if let Some(run) = run.as_ref()
                && run.claimed_by.is_some()
            {
                return self.fenced_upsert(root_id, relative_path, meta, scan_id);
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
            if run.is_none_or(|r| r.root_id != root_id) || latest != Some(scan_id) {
                return Ok(None);
            }
            return self.plain_upsert(root_id, relative_path, meta, Some((scan_id, generation)));
        }
        self.plain_upsert(root_id, relative_path, meta, None)
    }

    fn plain_upsert(
        &self,
        root_id: i64,
        relative_path: &str,
        meta: &MediaMeta,
        scan: Option<(i64, i64)>,
    ) -> Result<Option<MediaFile>> {
        if self.get_by_path(root_id, relative_path)?.is_none() {
            self.insert(root_id, relative_path, meta, scan)?;
        } else {
            self.0.conn.execute(
                "UPDATE media_files SET kind=?3, suffix=?4, size=?5, mtime_ns=?6, fingerprint=?7,
                   scanned_device=?8, scanned_inode=?9, scanned_ctime_ns=?10,
                   sidecar_fingerprint=?11, discovery_state=?12, last_seen_at=?13,
                   last_seen_scan_id=?14, last_seen_generation=?15
                 WHERE root_id=?1 AND relative_path=?2",
                rusqlite::params![
                    root_id,
                    relative_path,
                    meta.kind,
                    meta.suffix,
                    meta.size,
                    meta.mtime_ns,
                    meta.fingerprint,
                    meta.scanned_device,
                    meta.scanned_inode,
                    meta.scanned_ctime_ns,
                    meta.sidecar_fingerprint,
                    meta.state(),
                    self.0.clock.now(),
                    scan.map(|s| s.0),
                    scan.map(|s| s.1),
                ],
            )?;
        }
        self.get_by_path(root_id, relative_path)
    }

    /// The upsert of a claimed scheduled scan: authority is checked inside the statement, so a
    /// scheduler that lost its lease between check and write changes nothing.
    fn fenced_upsert(
        &self,
        root_id: i64,
        relative_path: &str,
        meta: &MediaMeta,
        scan_id: i64,
    ) -> Result<Option<MediaFile>> {
        let sql = format!(
            "INSERT INTO media_files
             (root_id, relative_path, kind, suffix, size, mtime_ns, fingerprint,
              scanned_device, scanned_inode, scanned_ctime_ns, sidecar_fingerprint, discovery_state,
              compliance_state, workflow_state, desired_action, last_seen_at, last_seen_scan_id,
              last_seen_generation)
             SELECT :root_id, :relative_path, :kind, :suffix, :size, :mtime_ns, :fingerprint,
                    :scanned_device, :scanned_inode, :scanned_ctime_ns, :sidecar_fingerprint,
                    :discovery_state, 'unknown', 'idle', 'none', :now, scan.id, scan.generation
             FROM scan_runs AS scan WHERE {CLAIMED_SCAN_AUTHORITY}
             ON CONFLICT(root_id, relative_path) DO UPDATE SET
                 kind=excluded.kind, suffix=excluded.suffix, size=excluded.size,
                 mtime_ns=excluded.mtime_ns, fingerprint=excluded.fingerprint,
                 scanned_device=excluded.scanned_device, scanned_inode=excluded.scanned_inode,
                 scanned_ctime_ns=excluded.scanned_ctime_ns,
                 sidecar_fingerprint=excluded.sidecar_fingerprint,
                 discovery_state=excluded.discovery_state, last_seen_at=excluded.last_seen_at,
                 last_seen_scan_id=excluded.last_seen_scan_id,
                 last_seen_generation=excluded.last_seen_generation
             WHERE EXISTS (SELECT 1 FROM scan_runs AS scan WHERE {CLAIMED_SCAN_AUTHORITY})
             RETURNING id"
        );
        let id: Option<i64> = self
            .0
            .conn
            .query_row(
                &sql,
                named_params! {
                    ":scan_id": scan_id, ":root_id": root_id, ":relative_path": relative_path,
                    ":kind": meta.kind, ":suffix": meta.suffix, ":size": meta.size,
                    ":mtime_ns": meta.mtime_ns, ":fingerprint": meta.fingerprint,
                    ":sidecar_fingerprint": meta.sidecar_fingerprint,
                    ":scanned_device": meta.scanned_device, ":scanned_inode": meta.scanned_inode,
                    ":scanned_ctime_ns": meta.scanned_ctime_ns,
                    ":discovery_state": meta.state(), ":now": self.0.clock.now(),
                },
                |r| r.get(0),
            )
            .optional()?;
        id.map_or(Ok(None), |id| self.get(id))
    }

    /// Remember which file a conversion produced and from which input.
    ///
    /// # Errors
    ///
    /// The media id is unknown.
    pub fn set_conversion(
        &self,
        media_id: i64,
        output_media_id: i64,
        input_fingerprint: &str,
    ) -> Result<()> {
        let changed = self.0.conn.execute(
            "UPDATE media_files SET conversion_output_media_id=?2, last_converted_input_fingerprint=?3
             WHERE id=?1",
            rusqlite::params![media_id, output_media_id, input_fingerprint],
        )?;
        if changed == 0 {
            return Err(Error::Invalid(format!("unknown media id: {media_id}")));
        }
        Ok(())
    }
}

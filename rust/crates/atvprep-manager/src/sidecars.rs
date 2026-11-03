//! Subtitle sidecars of a media file.

use rusqlite::{OptionalExtension, named_params};

use crate::Result;
use crate::models::Sidecar;
use crate::repo::Ctx;
use crate::scans::{CLAIMED_SCAN_AUTHORITY, Scans};

pub struct Sidecars<'a>(pub Ctx<'a>);

impl Sidecars<'_> {
    /// Insert or update a sidecar. With a `scan_id` the write needs that scan to be authoritative
    /// and to be the one that last saw the media file; otherwise `None`.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn upsert(
        &self,
        media_file_id: i64,
        relative_path: &str,
        fingerprint: &str,
        size: i64,
        mtime_ns: i64,
        scan_id: Option<i64>,
    ) -> Result<Option<Sidecar>> {
        if let Some(scan_id) = scan_id {
            let Some(scan) = Scans(self.0).get(scan_id)? else {
                return Ok(None);
            };
            if scan.claimed_by.is_some() {
                let sql = format!(
                    "INSERT INTO sidecars (media_file_id, relative_path, fingerprint, size, mtime_ns)
                     SELECT :media_file_id, :relative_path, :fingerprint, :size, :mtime_ns
                     FROM scan_runs AS scan JOIN media_files AS media ON media.id=:media_file_id
                     WHERE media.last_seen_scan_id=scan.id AND {CLAIMED_SCAN_AUTHORITY}
                     ON CONFLICT(media_file_id, relative_path) DO UPDATE SET
                         fingerprint=excluded.fingerprint, size=excluded.size,
                         mtime_ns=excluded.mtime_ns
                     WHERE EXISTS (SELECT 1 FROM scan_runs AS scan
                         JOIN media_files AS media ON media.id=:media_file_id
                         WHERE media.last_seen_scan_id=scan.id AND {CLAIMED_SCAN_AUTHORITY})
                     RETURNING id"
                );
                let id: Option<i64> = self
                    .0
                    .conn
                    .query_row(
                        &sql,
                        named_params! {
                            ":media_file_id": media_file_id, ":relative_path": relative_path,
                            ":fingerprint": fingerprint, ":size": size, ":mtime_ns": mtime_ns,
                            ":scan_id": scan_id, ":root_id": scan.root_id,
                            ":now": self.0.clock.now(),
                        },
                        |r| r.get(0),
                    )
                    .optional()?;
                return id.map_or(Ok(None), |id| self.get(id));
            }
            let seen_by: Option<Option<i64>> = self
                .0
                .conn
                .query_row(
                    "SELECT last_seen_scan_id FROM media_files WHERE id = ?1",
                    [media_file_id],
                    |r| r.get(0),
                )
                .optional()?;
            if seen_by != Some(Some(scan_id))
                || !Scans(self.0).is_authoritative(scan_id, scan.root_id)?
            {
                return Ok(None);
            }
        }
        self.0.conn.execute(
            "INSERT INTO sidecars (media_file_id, relative_path, fingerprint, size, mtime_ns)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(media_file_id, relative_path) DO UPDATE SET
                 fingerprint=excluded.fingerprint, size=excluded.size, mtime_ns=excluded.mtime_ns",
            rusqlite::params![media_file_id, relative_path, fingerprint, size, mtime_ns],
        )?;
        let id: i64 = self.0.conn.query_row(
            "SELECT id FROM sidecars WHERE media_file_id=?1 AND relative_path=?2",
            rusqlite::params![media_file_id, relative_path],
            |r| r.get(0),
        )?;
        self.get(id)
    }

    fn get(&self, id: i64) -> Result<Option<Sidecar>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT * FROM sidecars WHERE id = ?1",
                [id],
                Sidecar::from_row,
            )
            .optional()?)
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn list_for_media(&self, media_file_id: i64) -> Result<Vec<Sidecar>> {
        let mut statement = self
            .0
            .conn
            .prepare("SELECT * FROM sidecars WHERE media_file_id = ?1 ORDER BY relative_path")?;
        let rows = statement.query_map([media_file_id], Sidecar::from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

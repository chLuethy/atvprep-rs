//! Stored tool results and the compliance projection they feed.

use rusqlite::{OptionalExtension, named_params};
use serde_json::Value;

use crate::Result;
use crate::evaluate::evaluate_inspection;
use crate::models::Inspection;
use crate::repo::Ctx;
use crate::scans::{CLAIMED_SCAN_AUTHORITY, Scans};

pub struct Inspections<'a>(pub Ctx<'a>);

/// What a tool run produced.
pub struct NewInspection<'a> {
    pub source_fingerprint: &'a str,
    pub input_fingerprint: &'a str,
    pub tool_name: &'a str,
    pub tool_version: &'a str,
    pub result: &'a Value,
}

impl Inspections<'_> {
    fn get(&self, id: i64) -> Result<Option<Inspection>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT * FROM inspections WHERE id = ?1",
                [id],
                Inspection::from_row,
            )
            .optional()?)
    }

    /// Persist the compliance projection for the media file of `inspection`.
    ///
    /// A verify report is the only thing that can say clean, warning or blocker, while an ffprobe
    /// inspection can only say "unknown". So an ffprobe inspection must not replace a verify
    /// verdict for the same bytes: the verdict comes from the newest verify report whose
    /// fingerprint still matches the file, and only without one does the inspection decide.
    fn project(&self, inspection: &Inspection) -> Result<()> {
        let media: Option<String> = self
            .0
            .conn
            .query_row(
                "SELECT fingerprint FROM media_files WHERE id = ?1",
                [inspection.media_file_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(media_fingerprint) = media else {
            return Ok(());
        };
        let mut source = inspection.clone();
        if inspection.tool_name != "verify" {
            let verified: Option<i64> = self
                .0
                .conn
                .query_row(
                    "SELECT id FROM inspections
                     WHERE media_file_id=?1 AND tool_name='verify' AND input_fingerprint=?2
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    rusqlite::params![inspection.media_file_id, media_fingerprint],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(found) = verified.map(|id| self.get(id)).transpose()?.flatten() {
                source = found;
            }
        }
        let evaluation = evaluate_inspection(
            &source.tool_name,
            &source.result,
            &source.input_fingerprint,
            &media_fingerprint,
        );
        self.0.conn.execute(
            "UPDATE media_files SET compliance_state=?2, compliance_reasons=?3 WHERE id=?1",
            rusqlite::params![
                inspection.media_file_id,
                evaluation.state,
                serde_json::to_string(&evaluation.reasons).unwrap_or_else(|_| "[]".to_owned()),
            ],
        )?;
        Ok(())
    }

    /// Record an inspection and update the media file's compliance projection. With a `scan_id`
    /// the write needs that scan to be authoritative and the one that last saw the file.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn record(
        &self,
        media_file_id: i64,
        new: &NewInspection<'_>,
        scan_id: Option<i64>,
    ) -> Result<Option<Inspection>> {
        let now = self.0.clock.now();
        let text = new.result.to_string();
        if let Some(scan_id) = scan_id {
            let Some(scan) = Scans(self.0).get(scan_id)? else {
                return Ok(None);
            };
            if scan.claimed_by.is_some() {
                let sql = format!(
                    "INSERT INTO inspections
                     (media_file_id, source_fingerprint, input_fingerprint, tool_name, tool_version,
                      result, created_at)
                     SELECT :media_file_id, :source_fingerprint, :input_fingerprint, :tool_name,
                            :tool_version, :result, :now
                     FROM scan_runs AS scan JOIN media_files AS media ON media.id=:media_file_id
                     WHERE media.last_seen_scan_id=scan.id AND {CLAIMED_SCAN_AUTHORITY}
                     RETURNING id"
                );
                let id: Option<i64> = self
                    .0
                    .conn
                    .query_row(
                        &sql,
                        named_params! {
                            ":media_file_id": media_file_id,
                            ":source_fingerprint": new.source_fingerprint,
                            ":input_fingerprint": new.input_fingerprint,
                            ":tool_name": new.tool_name, ":tool_version": new.tool_version,
                            ":result": text, ":scan_id": scan_id, ":root_id": scan.root_id,
                            ":now": now,
                        },
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(inspection) = id.map(|id| self.get(id)).transpose()?.flatten() else {
                    return Ok(None);
                };
                self.project(&inspection)?;
                return Ok(Some(inspection));
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
            "INSERT INTO inspections
             (media_file_id, source_fingerprint, input_fingerprint, tool_name, tool_version,
              result, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                media_file_id,
                new.source_fingerprint,
                new.input_fingerprint,
                new.tool_name,
                new.tool_version,
                text,
                now
            ],
        )?;
        let inspection = self.get(self.0.conn.last_insert_rowid())?;
        if let Some(inspection) = &inspection {
            self.project(inspection)?;
        }
        Ok(inspection)
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn latest(&self, media_file_id: i64) -> Result<Option<Inspection>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT * FROM inspections WHERE media_file_id = ?1
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                [media_file_id],
                Inspection::from_row,
            )
            .optional()?)
    }

    /// Reproject every media file from its latest stored inspection; idempotent and runs no media
    /// tool.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn backfill_compliance(&self) -> Result<usize> {
        let ids: Vec<i64> = {
            let mut statement = self
                .0
                .conn
                .prepare("SELECT id FROM media_files ORDER BY id")?;
            let rows = statement.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for id in &ids {
            if let Some(inspection) = self.latest(*id)? {
                self.project(&inspection)?;
            } else {
                self.0.conn.execute(
                    "UPDATE media_files SET compliance_state='unknown', compliance_reasons=?2 WHERE id=?1",
                    rusqlite::params![
                        id,
                        serde_json::json!(["No inspection is available for this media file."])
                            .to_string()
                    ],
                )?;
            }
        }
        Ok(ids.len())
    }
}

//! Immutable, versioned input snapshots for job items (port of `atvprep_manager/worker/snapshots.py`
//! and `InputSnapshotRepository`).
//!
//! Queueing and execution must reference exactly the same input. A snapshot captures the
//! comparable state of a media file, its sidecars, the latest inspection and the settings revision.

use rusqlite::OptionalExtension;
use serde_json::{Value, json};

use crate::inspections::Inspections;
use crate::media::Media;
use crate::repo::Ctx;
use crate::settings::Settings;
use crate::sidecars::Sidecars;
use crate::{Error, Result};

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputSnapshot {
    pub schema_version: i64,
    pub media_id: i64,
    pub file_size: i64,
    pub file_mtime_ns: i64,
    pub file_fingerprint: String,
    pub file_device: i64,
    pub file_inode: i64,
    pub file_ctime_ns: i64,
    pub sidecar_fingerprint: String,
    pub inspection_input_fingerprint: String,
    pub settings_revision: i64,
}

impl InputSnapshot {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schema_version": self.schema_version,
            "media_id": self.media_id,
            "file_size": self.file_size,
            "file_mtime_ns": self.file_mtime_ns,
            "file_fingerprint": self.file_fingerprint,
            "file_device": self.file_device,
            "file_inode": self.file_inode,
            "file_ctime_ns": self.file_ctime_ns,
            "sidecar_fingerprint": self.sidecar_fingerprint,
            "inspection_input_fingerprint": self.inspection_input_fingerprint,
            "settings_revision": self.settings_revision,
        })
    }

    /// # Errors
    ///
    /// A field is missing or has the wrong type.
    pub fn from_json(data: &Value) -> Result<Self> {
        let int = |name: &str| {
            data.get(name)
                .and_then(Value::as_i64)
                .ok_or_else(|| Error::Invalid(format!("snapshot field {name} is missing")))
        };
        let text = |name: &str| {
            data.get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| Error::Invalid(format!("snapshot field {name} is missing")))
        };
        Ok(Self {
            schema_version: int("schema_version")?,
            media_id: int("media_id")?,
            file_size: int("file_size")?,
            file_mtime_ns: int("file_mtime_ns")?,
            file_fingerprint: text("file_fingerprint")?,
            file_device: int("file_device")?,
            file_inode: int("file_inode")?,
            file_ctime_ns: int("file_ctime_ns")?,
            sidecar_fingerprint: text("sidecar_fingerprint")?,
            inspection_input_fingerprint: text("inspection_input_fingerprint")?,
            settings_revision: int("settings_revision")?,
        })
    }
}

/// Divergence reasons between a stored and the current snapshot; empty means unchanged. A missing
/// media row is reported as `media.missing`.
#[must_use]
pub fn compare_snapshot(stored: &InputSnapshot, current: Option<&InputSnapshot>) -> Vec<String> {
    let Some(current) = current else {
        return vec!["media.missing".to_owned()];
    };
    let checks = [
        (stored.file_size != current.file_size, "file.size"),
        (
            stored.file_mtime_ns != current.file_mtime_ns,
            "file.mtime_ns",
        ),
        (
            stored.file_fingerprint != current.file_fingerprint,
            "file.fingerprint",
        ),
        (
            (stored.file_device, stored.file_inode) != (current.file_device, current.file_inode),
            "file.identity",
        ),
        (
            stored.file_ctime_ns != current.file_ctime_ns,
            "file.ctime_ns",
        ),
        (
            stored.sidecar_fingerprint != current.sidecar_fingerprint,
            "sidecar_fingerprint",
        ),
        (
            stored.inspection_input_fingerprint != current.inspection_input_fingerprint,
            "inspection.input_fingerprint",
        ),
        (
            stored.settings_revision != current.settings_revision,
            "settings.revision",
        ),
    ];
    checks
        .iter()
        .filter(|(differs, _)| *differs)
        .map(|(_, reason)| (*reason).to_owned())
        .collect()
}

/// The snapshot of a media file as it is now (legacy nullable identity counts as zero).
///
/// # Errors
///
/// SQLite fails.
pub fn build_for_media(ctx: Ctx<'_>, media_id: i64) -> Result<Option<InputSnapshot>> {
    let Some(media) = Media(ctx).get(media_id)? else {
        return Ok(None);
    };
    let sidecar_fingerprint = Sidecars(ctx)
        .list_for_media(media_id)?
        .iter()
        .map(|s| format!("{}:{}", s.relative_path, s.fingerprint))
        .collect::<Vec<_>>()
        .join("|");
    let inspection_input_fingerprint = Inspections(ctx)
        .latest(media_id)?
        .map(|i| i.input_fingerprint)
        .unwrap_or_default();
    Ok(Some(InputSnapshot {
        schema_version: SCHEMA_VERSION,
        media_id: media.id,
        file_size: media.size,
        file_mtime_ns: media.mtime_ns,
        file_fingerprint: media.fingerprint,
        file_device: media.scanned_device.unwrap_or(0),
        file_inode: media.scanned_inode.unwrap_or(0),
        file_ctime_ns: media.scanned_ctime_ns.unwrap_or(0),
        sidecar_fingerprint,
        inspection_input_fingerprint,
        settings_revision: Settings(ctx).current_revision()?,
    }))
}

pub struct InputSnapshots<'a>(pub Ctx<'a>);

impl InputSnapshots<'_> {
    /// Store the snapshot of an item once; a second call returns the first.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn record(&self, job_item_id: i64, snapshot: &Value) -> Result<Value> {
        if let Some(existing) = self.get_for_item(job_item_id)? {
            return Ok(existing);
        }
        self.0.conn.execute(
            "INSERT INTO input_snapshots(job_item_id, snapshot_version, snapshot_json, created_at)
             VALUES (?1, 1, ?2, ?3)",
            rusqlite::params![job_item_id, snapshot.to_string(), self.0.clock.now()],
        )?;
        Ok(snapshot.clone())
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn get_for_item(&self, job_item_id: i64) -> Result<Option<Value>> {
        let text: Option<String> = self
            .0
            .conn
            .query_row(
                "SELECT snapshot_json FROM input_snapshots WHERE job_item_id = ?1",
                [job_item_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(text.map(|t| serde_json::from_str(&t).unwrap_or(Value::Null)))
    }

    /// The snapshot of the item's media as it is now.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn build_current(&self, item_id: i64) -> Result<Option<InputSnapshot>> {
        let media: Option<i64> = self
            .0
            .conn
            .query_row(
                "SELECT media_file_id FROM job_items WHERE id = ?1",
                [item_id],
                |r| r.get(0),
            )
            .optional()?;
        media.map_or(Ok(None), |id| build_for_media(self.0, id))
    }

    /// Compare an item's stored snapshot with live state: empty when unchanged, otherwise the
    /// reasons. Missing stored state is a reason (`snapshot.missing`), not an error.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn check_item_not_stale(&self, item_id: i64) -> Result<Vec<String>> {
        let Some(stored) = self.get_for_item(item_id)? else {
            return Ok(vec!["snapshot.missing".to_owned()]);
        };
        let stored = InputSnapshot::from_json(&stored)?;
        Ok(compare_snapshot(
            &stored,
            self.build_current(item_id)?.as_ref(),
        ))
    }
}

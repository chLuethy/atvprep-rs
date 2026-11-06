//! Rows as the repositories return them.

use rusqlite::Row;

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoot {
    pub id: i64,
    pub name: String,
    pub canonical_path: String,
    pub enabled: bool,
}

impl LibraryRoot {
    pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            canonical_path: row.get("canonical_path")?,
            enabled: row.get("enabled")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRun {
    pub id: i64,
    pub root_id: i64,
    pub generation: i64,
    pub trigger: String,
    pub status: String,
    pub claimed_by: Option<String>,
    pub attempt: i64,
    pub status_version: i64,
    pub fencing_token: i64,
    pub finished_at: Option<String>,
}

impl ScanRun {
    pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            root_id: row.get("root_id")?,
            generation: row.get("generation")?,
            trigger: row.get("trigger")?,
            status: row.get("status")?,
            claimed_by: row.get("claimed_by")?,
            attempt: row.get("attempt")?,
            status_version: row.get("status_version")?,
            fencing_token: row.get("fencing_token")?,
            finished_at: row.get("finished_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFile {
    pub id: i64,
    pub root_id: i64,
    pub relative_path: String,
    pub kind: String,
    pub suffix: String,
    pub size: i64,
    pub mtime_ns: i64,
    pub fingerprint: String,
    pub scanned_device: Option<i64>,
    pub scanned_inode: Option<i64>,
    pub scanned_ctime_ns: Option<i64>,
    pub sidecar_fingerprint: String,
    pub discovery_state: String,
    pub compliance_state: String,
    pub compliance_reasons: Vec<String>,
    pub workflow_state: String,
    pub desired_action: String,
    pub last_seen_at: String,
    pub last_seen_scan_id: Option<i64>,
    pub last_seen_generation: Option<i64>,
    pub conversion_output_media_id: Option<i64>,
    pub last_converted_input_fingerprint: Option<String>,
    pub dismissed_at: Option<String>,
}

impl MediaFile {
    pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let reasons: String = row.get("compliance_reasons")?;
        Ok(Self {
            id: row.get("id")?,
            root_id: row.get("root_id")?,
            relative_path: row.get("relative_path")?,
            kind: row.get("kind")?,
            suffix: row.get("suffix")?,
            size: row.get("size")?,
            mtime_ns: row.get("mtime_ns")?,
            fingerprint: row.get("fingerprint")?,
            scanned_device: row.get("scanned_device")?,
            scanned_inode: row.get("scanned_inode")?,
            scanned_ctime_ns: row.get("scanned_ctime_ns")?,
            sidecar_fingerprint: row.get("sidecar_fingerprint")?,
            discovery_state: row.get("discovery_state")?,
            compliance_state: row.get("compliance_state")?,
            compliance_reasons: serde_json::from_str(&reasons).unwrap_or_default(),
            workflow_state: row.get("workflow_state")?,
            desired_action: row.get("desired_action")?,
            last_seen_at: row.get("last_seen_at")?,
            last_seen_scan_id: row.get("last_seen_scan_id")?,
            last_seen_generation: row.get("last_seen_generation")?,
            conversion_output_media_id: row.get("conversion_output_media_id")?,
            last_converted_input_fingerprint: row.get("last_converted_input_fingerprint")?,
            dismissed_at: row.get("dismissed_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidecar {
    pub id: i64,
    pub media_file_id: i64,
    pub relative_path: String,
    pub fingerprint: String,
    pub size: i64,
    pub mtime_ns: i64,
}

impl Sidecar {
    pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            media_file_id: row.get("media_file_id")?,
            relative_path: row.get("relative_path")?,
            fingerprint: row.get("fingerprint")?,
            size: row.get("size")?,
            mtime_ns: row.get("mtime_ns")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub id: i64,
    pub media_file_id: i64,
    pub source_fingerprint: String,
    pub input_fingerprint: String,
    pub tool_name: String,
    pub tool_version: String,
    pub result: serde_json::Value,
    pub created_at: String,
}

impl Inspection {
    pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let result: String = row.get("result")?;
        Ok(Self {
            id: row.get("id")?,
            media_file_id: row.get("media_file_id")?,
            source_fingerprint: row.get("source_fingerprint")?,
            input_fingerprint: row.get("input_fingerprint")?,
            tool_name: row.get("tool_name")?,
            tool_version: row.get("tool_version")?,
            result: serde_json::from_str(&result).unwrap_or(serde_json::Value::Null),
            created_at: row.get("created_at")?,
        })
    }
}

/// The scanner's description of one file.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MediaMeta {
    pub kind: String,
    pub suffix: String,
    pub size: i64,
    pub mtime_ns: i64,
    pub fingerprint: String,
    pub sidecar_fingerprint: String,
    pub scanned_device: Option<i64>,
    pub scanned_inode: Option<i64>,
    pub scanned_ctime_ns: Option<i64>,
    /// `present` unless the scanner says otherwise; empty means `present`.
    pub discovery_state: String,
}

impl MediaMeta {
    /// Python's `_require_scan_identity`: a complete, non-negative device/inode/ctime.
    pub(crate) fn require_identity(&self) -> Result<()> {
        let complete = [
            self.scanned_device,
            self.scanned_inode,
            self.scanned_ctime_ns,
        ]
        .iter()
        .all(|v| v.is_some_and(|n| n >= 0));
        if complete {
            Ok(())
        } else {
            Err(crate::Error::Invalid(
                "new media records require a complete valid scan identity".to_owned(),
            ))
        }
    }

    pub(crate) fn state(&self) -> &str {
        if self.discovery_state.is_empty() {
            "present"
        } else {
            &self.discovery_state
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: i64,
    pub action: String,
    pub status: crate::state_machine::JobStatus,
    pub trigger: String,
    pub origin_job_id: Option<i64>,
    pub settings_revision: i64,
    pub status_version: i64,
    pub settings_snapshot: serde_json::Value,
    pub requested_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub cancel_requested_at: Option<String>,
    pub cleared_at: Option<String>,
    pub error: Option<String>,
    pub total_count: i64,
    pub succeeded_count: i64,
    pub failed_count: i64,
    pub cancelled_count: i64,
}

fn json_column(row: &Row<'_>, name: &str) -> rusqlite::Result<serde_json::Value> {
    let text: String = row.get(name)?;
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
}

impl Job {
    pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            action: row.get("action")?,
            status: row.get("status")?,
            trigger: row.get("trigger")?,
            origin_job_id: row.get("origin_job_id")?,
            settings_revision: row.get("settings_revision")?,
            status_version: row.get("status_version")?,
            settings_snapshot: json_column(row, "settings_snapshot_json")?,
            requested_at: row.get("requested_at")?,
            started_at: row.get("started_at")?,
            finished_at: row.get("finished_at")?,
            cancel_requested_at: row.get("cancel_requested_at")?,
            cleared_at: row.get("cleared_at")?,
            error: row.get("error")?,
            total_count: row.get("total_count")?,
            succeeded_count: row.get("succeeded_count")?,
            failed_count: row.get("failed_count")?,
            cancelled_count: row.get("cancelled_count")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobItem {
    pub id: i64,
    pub job_id: i64,
    pub media_file_id: i64,
    pub action: String,
    pub position: i64,
    pub origin_item_id: Option<i64>,
    pub status: crate::state_machine::ItemStatus,
    pub status_version: i64,
    pub attempt: i64,
    pub claimed_by: Option<String>,
    pub lease_expires_at: Option<String>,
    pub heartbeat_at: Option<String>,
    pub executor_fencing_token: Option<i64>,
    pub input_snapshot: serde_json::Value,
    pub settings_snapshot: serde_json::Value,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub output_media_file_id: Option<i64>,
    pub log_path: Option<String>,
    pub log_tail: Option<String>,
    pub progress: Option<i64>,
}

impl JobItem {
    pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            job_id: row.get("job_id")?,
            media_file_id: row.get("media_file_id")?,
            action: row.get("action")?,
            position: row.get("position")?,
            origin_item_id: row.get("origin_item_id")?,
            status: row.get("status")?,
            status_version: row.get("status_version")?,
            attempt: row.get("attempt")?,
            claimed_by: row.get("claimed_by")?,
            lease_expires_at: row.get("lease_expires_at")?,
            heartbeat_at: row.get("heartbeat_at")?,
            executor_fencing_token: row.get("executor_fencing_token")?,
            input_snapshot: json_column(row, "input_snapshot_json")?,
            settings_snapshot: json_column(row, "settings_snapshot_json")?,
            started_at: row.get("started_at")?,
            finished_at: row.get("finished_at")?,
            exit_code: row.get("exit_code")?,
            error_code: row.get("error_code")?,
            error_detail: row.get("error_detail")?,
            output_media_file_id: row.get("output_media_file_id")?,
            log_path: row.get("log_path")?,
            log_tail: row.get("log_tail")?,
            progress: row.get("progress")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: i64,
    pub job_id: Option<i64>,
    pub item_id: Option<i64>,
    pub topic: String,
    pub entity_type: String,
    pub entity_id: i64,
    pub payload: serde_json::Value,
    pub created_at: String,
    pub expires_at: String,
}

impl Event {
    pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            job_id: row.get("job_id")?,
            item_id: row.get("item_id")?,
            topic: row.get("topic")?,
            entity_type: row.get("entity_type")?,
            entity_id: row.get("entity_id")?,
            payload: json_column(row, "payload_json")?,
            created_at: row.get("created_at")?,
            expires_at: row.get("expires_at")?,
        })
    }
}

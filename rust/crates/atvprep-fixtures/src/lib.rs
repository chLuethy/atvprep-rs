//! Test data in the manager's own words: a library, a file with a warning, a job with two items.
//!
//! The builder lays data down through the manager's repositories, so it satisfies the same
//! invariants as real data (a file with a verdict has the matching state, a job has one queued item
//! per file). A new column with a default touches this crate only, not the tests.
//!
//! Three things have no repository, and the builder writes them directly, in one place each:
//! schedules (the API creates them), the status of a job and its items (the worker's state machine
//! needs leases and a running process), and the verdict of a file that is not an MP4 (verify cannot
//! judge another container). [`Fixtures::sql`] is the explicit way out for historical or invalid rows.

use std::sync::Arc;

use atvprep_manager::clock::{Clock, FixedClock, SystemClock};
use atvprep_manager::inspections::{Inspections, NewInspection};
use atvprep_manager::jobs::{Jobs, NewJob};
use atvprep_manager::library::LibraryRoots;
use atvprep_manager::media::Media;
use atvprep_manager::models::MediaMeta;
use atvprep_manager::repo::Ctx;
use atvprep_manager::scans::Scans;
use atvprep_manager::sidecars::Sidecars;
use rusqlite::Connection;
use rusqlite::types::Value as Sql;
use serde_json::{Value, json};

/// What verify says about a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    Warning,
    Blocker,
}

impl Verdict {
    const fn severity(self) -> Option<&'static str> {
        match self {
            Self::Clean => None,
            Self::Warning => Some("warning"),
            Self::Blocker => Some("blocker"),
        }
    }
}

/// Microseconds since the epoch of a stored time (`YYYY-MM-DD HH:MM:SS[.ffffff]`, UTC).
///
/// # Panics
///
/// Panics when `text` is not in that form; this is test tooling.
#[must_use]
pub fn micros(text: &str) -> i64 {
    let parsed = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
        .unwrap_or_else(|_| panic!("not a stored time: {text}"));
    parsed.and_utc().timestamp_micros()
}

/// The builder. It borrows a migrated connection and lives as long as the test needs it.
pub struct Fixtures<'a> {
    conn: &'a Connection,
    clock: Arc<dyn Clock>,
}

impl<'a> Fixtures<'a> {
    /// Fixtures on `conn`, stamped with the system time.
    #[must_use]
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            clock: Arc::new(SystemClock),
        }
    }

    /// The same fixtures, stamping what they create with `stored_time` (see [`micros`]).
    #[must_use]
    pub fn at(&self, stored_time: &str) -> Self {
        Self {
            conn: self.conn,
            clock: Arc::new(FixedClock(micros(stored_time))),
        }
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            conn: self.conn,
            clock: &*self.clock,
        }
    }

    /// A library named `name`, at `/<name in lower case>`. Returns its id.
    ///
    /// # Panics
    ///
    /// Panics when the library cannot be created; this is test tooling.
    #[must_use]
    pub fn library(&self, name: &str) -> i64 {
        self.library_at(name, &format!("/{}", name.to_lowercase()))
    }

    /// A library named `name` whose private path is `path`. Returns its id.
    ///
    /// # Panics
    ///
    /// Panics when the library cannot be created; this is test tooling.
    #[must_use]
    pub fn library_at(&self, name: &str, path: &str) -> i64 {
        LibraryRoots(self.ctx()).create(name, path).unwrap().id
    }

    /// A file at `relative_path` of library `root`; call [`NewFile::create`] to lay it down.
    #[must_use]
    pub fn file(&self, root: i64, relative_path: &str) -> NewFile<'_, 'a> {
        NewFile {
            fx: self,
            root,
            path: relative_path.to_owned(),
            fingerprint: None,
            size: 1,
            verdict: None,
            encode: None,
            discovery_state: "present",
            dismissed_at: None,
        }
    }

    /// A job of `action` with one queued item per file; call [`NewJobSpec::create`].
    #[must_use]
    pub fn job<'f>(&'f self, action: &'f str, files: &'f [i64]) -> NewJobSpec<'f, 'a> {
        NewJobSpec {
            fx: self,
            action,
            files,
            trigger: "automatic",
            status: None,
            settings: json!({}),
            requested_at: None,
        }
    }

    /// A schedule; call [`NewSchedule::create`].
    #[must_use]
    pub fn schedule(&self, name: &str, action: &str) -> NewSchedule<'_, 'a> {
        NewSchedule {
            fx: self,
            name: name.to_owned(),
            action: action.to_owned(),
            columns: Vec::new(),
        }
    }

    /// A scan of `root` in `generation` that ended as `status` (`running`, `succeeded`, `failed`).
    ///
    /// # Panics
    ///
    /// Panics when the scan cannot be created; this is test tooling.
    #[must_use]
    pub fn scan(&self, root: i64, generation: i64, status: &str) -> i64 {
        let scan = Scans(self.ctx())
            .start(root, generation, "scheduled")
            .unwrap();
        if status != "running" {
            self.conn
                .execute(
                    "UPDATE scan_runs SET status=?2 WHERE id=?1",
                    rusqlite::params![scan.id, status],
                )
                .unwrap();
        }
        scan.id
    }

    /// A current runtime setting.
    ///
    /// # Panics
    ///
    /// Panics when SQLite fails; this is test tooling.
    pub fn setting(&self, key: &str, value: &Value, value_type: &str, revision: i64) {
        Jobs(self.ctx())
            .set_runtime_setting(key, value, value_type, revision)
            .unwrap();
    }

    /// A sidecar of `file`. Returns its id.
    ///
    /// # Panics
    ///
    /// Panics when the sidecar cannot be stored; this is test tooling.
    #[must_use]
    pub fn sidecar(&self, file: i64, relative_path: &str, fingerprint: &str) -> i64 {
        Sidecars(self.ctx())
            .upsert(file, relative_path, fingerprint, 1, 1, None)
            .unwrap()
            .unwrap()
            .id
    }

    /// A stored tool result of `file` (`ffprobe` or `verify`) for exactly the bytes `input` names.
    /// Returns its id.
    ///
    /// # Panics
    ///
    /// Panics when the inspection cannot be stored; this is test tooling.
    #[must_use]
    pub fn inspection(
        &self,
        file: i64,
        tool: &str,
        input_fingerprint: &str,
        result: &Value,
    ) -> i64 {
        Inspections(self.ctx())
            .record(
                file,
                &NewInspection {
                    source_fingerprint: input_fingerprint,
                    input_fingerprint,
                    tool_name: tool,
                    tool_version: "1",
                    result,
                },
                None,
            )
            .unwrap()
            .unwrap()
            .id
    }

    /// An event of `job` (and optionally of one of its items), stamped with the fixtures' time.
    /// Returns its id.
    ///
    /// # Panics
    ///
    /// Panics when the job is unknown; this is test tooling.
    #[must_use]
    pub fn event(
        &self,
        job: i64,
        item: Option<i64>,
        topic: &str,
        entity_type: &str,
        payload: &Value,
    ) -> i64 {
        atvprep_manager::events::Events(self.ctx())
            .append(job, topic, entity_type, item.unwrap_or(job), payload, item)
            .unwrap()
            .id
    }

    /// Raw SQL, for historical or invalid rows the repositories would refuse.
    ///
    /// # Panics
    ///
    /// Panics when the SQL fails; this is test tooling.
    pub fn sql(&self, sql: &str) {
        self.conn.execute_batch(sql).unwrap();
    }
}

/// A file to lay down.
pub struct NewFile<'f, 'a> {
    fx: &'f Fixtures<'a>,
    root: i64,
    path: String,
    fingerprint: Option<String>,
    size: i64,
    verdict: Option<Verdict>,
    encode: Option<(&'static str, &'static str)>,
    discovery_state: &'static str,
    dismissed_at: Option<String>,
}

impl NewFile<'_, '_> {
    /// The content fingerprint; by default the path, so files differ.
    #[must_use]
    pub fn fingerprint(mut self, fingerprint: &str) -> Self {
        self.fingerprint = Some(fingerprint.to_owned());
        self
    }

    #[must_use]
    pub const fn size(mut self, size: i64) -> Self {
        self.size = size;
        self
    }

    /// What verify found. Without it the file is unknown.
    #[must_use]
    pub const fn verdict(mut self, verdict: Verdict) -> Self {
        self.verdict = Some(verdict);
        self
    }

    /// The Kodierbedarf (`none`, `audio`, `video`) and Änderungsart (`none`, `patch`, `remux`,
    /// `audio`, `reencode`) the last check found.
    #[must_use]
    pub const fn encode(mut self, need: &'static str, change: &'static str) -> Self {
        self.encode = Some((need, change));
        self
    }

    /// The file is gone from the library.
    #[must_use]
    pub const fn missing(mut self) -> Self {
        self.discovery_state = "missing";
        self
    }

    /// The operator hid the missing file at `stored_time`.
    #[must_use]
    pub fn dismissed_at(mut self, stored_time: &str) -> Self {
        self.dismissed_at = Some(stored_time.to_owned());
        self
    }

    /// Lay the file down. Returns its id.
    ///
    /// # Panics
    ///
    /// Panics when a repository refuses the data; this is test tooling.
    #[must_use]
    pub fn create(self) -> i64 {
        let suffix = self
            .path
            .rfind('.')
            .map_or(String::new(), |dot| self.path[dot..].to_owned());
        let fingerprint = self.fingerprint.unwrap_or_else(|| self.path.clone());
        let meta = MediaMeta {
            kind: "video".to_owned(),
            suffix: suffix.clone(),
            size: self.size,
            mtime_ns: 1,
            fingerprint: fingerprint.clone(),
            sidecar_fingerprint: String::new(),
            scanned_device: Some(1),
            scanned_inode: Some(1),
            scanned_ctime_ns: Some(1),
            discovery_state: self.discovery_state.to_owned(),
        };
        let id = Media(self.fx.ctx())
            .create(self.root, &self.path, &meta)
            .unwrap()
            .id;
        if self.verdict.is_some() || self.encode.is_some() {
            self.fx
                .judge(id, &suffix, &fingerprint, self.verdict, self.encode);
        }
        if let Some(at) = self.dismissed_at {
            self.fx
                .conn
                .execute(
                    "UPDATE media_files SET dismissed_at=?2 WHERE id=?1",
                    rusqlite::params![id, at],
                )
                .unwrap();
        }
        id
    }
}

impl Fixtures<'_> {
    /// Store what verify found. An MP4 gets a verify report and the manager projects it; verify
    /// cannot judge another container, so there the stored verdict is set directly.
    fn judge(
        &self,
        id: i64,
        suffix: &str,
        fingerprint: &str,
        verdict: Option<Verdict>,
        encode: Option<(&str, &str)>,
    ) {
        let verdict = verdict.unwrap_or(Verdict::Clean);
        let violations: Vec<Value> = verdict
            .severity()
            .map(|severity| json!({"severity": severity, "rule": "video-reencode", "message": "required"}))
            .into_iter()
            .collect();
        let mp4 = matches!(suffix.to_lowercase().as_str(), ".mp4" | ".m4v" | ".mov");
        if mp4 {
            let mut report = json!({"violations": violations});
            if let Some((need, change)) = encode {
                report["encode_need"] = json!(need);
                report["planned_change"] = json!(change);
            }
            let _ = self.inspection(id, "verify", fingerprint, &report);
            return;
        }
        let reasons: Vec<String> = verdict.severity().map_or_else(
            || vec!["No compliance violations reported by verify.".to_owned()],
            |severity| vec![format!("[{severity}] video-reencode: required")],
        );
        let state = match verdict {
            Verdict::Clean => "clean",
            Verdict::Warning => "warning",
            Verdict::Blocker => "blocker",
        };
        self.conn
            .execute(
                "UPDATE media_files SET compliance_state=?2, compliance_reasons=?3 WHERE id=?1",
                rusqlite::params![id, state, serde_json::to_string(&reasons).unwrap()],
            )
            .unwrap();
        if let Some((need, change)) = encode {
            self.conn
                .execute(
                    "UPDATE media_files SET encode_need=?2, planned_change=?3 WHERE id=?1",
                    rusqlite::params![id, need, change],
                )
                .unwrap();
        }
    }
}

/// A job to lay down.
pub struct NewJobSpec<'f, 'a> {
    fx: &'f Fixtures<'a>,
    action: &'f str,
    files: &'f [i64],
    trigger: &'f str,
    status: Option<&'f str>,
    settings: Value,
    requested_at: Option<&'f str>,
}

impl<'f> NewJobSpec<'f, '_> {
    /// `automatic` (the default), `manual` or `retry`.
    #[must_use]
    pub const fn trigger(mut self, trigger: &'f str) -> Self {
        self.trigger = trigger;
        self
    }

    /// The job and all its items are in this state (the default is queued).
    #[must_use]
    pub const fn status(mut self, status: &'f str) -> Self {
        self.status = Some(status);
        self
    }

    #[must_use]
    pub fn settings(mut self, settings: Value) -> Self {
        self.settings = settings;
        self
    }

    #[must_use]
    pub const fn requested_at(mut self, stored_time: &'f str) -> Self {
        self.requested_at = Some(stored_time);
        self
    }

    /// Lay the job down. Returns its id.
    ///
    /// # Panics
    ///
    /// Panics when a repository refuses the data; this is test tooling.
    #[must_use]
    pub fn create(self) -> i64 {
        let mut new = NewJob::new(self.action, self.files);
        new.trigger = self.trigger;
        new.settings_snapshot = self.settings;
        let job = Jobs(self.fx.ctx()).create(&new).unwrap().id;
        if let Some(status) = self.status {
            self.fx
                .conn
                .execute_batch(&format!(
                    "UPDATE jobs SET status='{status}' WHERE id={job};
                     UPDATE job_items SET status='{status}' WHERE job_id={job};"
                ))
                .unwrap();
        }
        if let Some(at) = self.requested_at {
            self.fx
                .conn
                .execute(
                    "UPDATE jobs SET requested_at=?2 WHERE id=?1",
                    rusqlite::params![job, at],
                )
                .unwrap();
        }
        job
    }
}

/// A schedule to lay down. Columns not set keep the table's defaults.
pub struct NewSchedule<'f, 'a> {
    fx: &'f Fixtures<'a>,
    name: String,
    action: String,
    columns: Vec<(&'static str, Sql)>,
}

impl NewSchedule<'_, '_> {
    fn set(mut self, column: &'static str, value: impl Into<Sql>) -> Self {
        self.columns.retain(|(name, _)| *name != column);
        self.columns.push((column, value.into()));
        self
    }

    #[must_use]
    pub fn library(self, root: i64) -> Self {
        self.set("root_id", root)
    }

    #[must_use]
    pub fn compliance_state(self, state: &str) -> Self {
        self.set("compliance_state", state.to_owned())
    }

    #[must_use]
    pub fn days_mask(self, mask: i64) -> Self {
        self.set("days_mask", mask)
    }

    #[must_use]
    pub fn time_of_day(self, time: &str) -> Self {
        self.set("time_of_day", time.to_owned())
    }

    #[must_use]
    pub fn max_files(self, max_files: i64) -> Self {
        self.set("max_files", max_files)
    }

    #[must_use]
    pub fn enabled(self, enabled: bool) -> Self {
        self.set("enabled", i64::from(enabled))
    }

    #[must_use]
    pub fn armed_at(self, stored_time: &str) -> Self {
        self.set("armed_at", stored_time.to_owned())
    }

    /// Lay the schedule down. Returns its id.
    ///
    /// # Panics
    ///
    /// Panics when the table refuses the data; this is test tooling.
    #[must_use]
    pub fn create(self) -> i64 {
        let mut names = vec!["name", "action", "days_mask", "time_of_day"];
        let mut values: Vec<Sql> = vec![
            self.name.into(),
            self.action.into(),
            1_i64.into(),
            "12:30".to_owned().into(),
        ];
        for (column, value) in self.columns {
            if let Some(at) = names.iter().position(|name| *name == column) {
                values[at] = value;
            } else {
                names.push(column);
                values.push(value);
            }
        }
        let placeholders = vec!["?"; names.len()].join(",");
        self.fx
            .conn
            .execute(
                &format!(
                    "INSERT INTO schedules({}) VALUES({placeholders})",
                    names.join(",")
                ),
                rusqlite::params_from_iter(values),
            )
            .unwrap();
        self.fx.conn.last_insert_rowid()
    }
}

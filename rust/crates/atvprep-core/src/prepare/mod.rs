//! `prepare`: bring one file to what Apple TV plays, by the cheapest route that is safe.

pub mod checks;
pub mod ffmpeg;
pub mod paths;
pub mod pipeline;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::escalation::Level;
use crate::pipeline_postfix::{RunContext, quarantine_output};
pub use crate::stages::Outcome;
use crate::verify::VerifyReport;

/// What one `prepare` run is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareOptions {
    pub output: Option<PathBuf>,
    pub force: bool,
    pub dry_run: bool,
    pub delete_original: bool,
    pub prefer_hevc10: bool,
    pub crf: u32,
    pub keep_av1: bool,
    pub fix_dts: bool,
    pub quarantine_dir: Option<PathBuf>,
    pub verbose: bool,
    pub extra_sidecars: Vec<PathBuf>,
    /// Refuse any run whose ffmpeg command would encode video or audio.
    pub copy_only: bool,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            output: None,
            force: false,
            dry_run: false,
            delete_original: false,
            prefer_hevc10: false,
            crf: 18,
            keep_av1: false,
            fix_dts: false,
            quarantine_dir: None,
            verbose: false,
            extra_sidecars: Vec::new(),
            copy_only: false,
        }
    }
}

/// Why a run failed; the tags are part of the machine-readable output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Analysis,
    Ffmpeg,
    NoOutput,
    Blockers,
    Unsafe,
    Transcode,
}

impl ErrorKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::Ffmpeg => "ffmpeg",
            Self::NoOutput => "no_output",
            Self::Blockers => "blockers",
            Self::Unsafe => "unsafe",
            Self::Transcode => "transcode",
        }
    }
}

/// How one file ended.
#[derive(Debug, Clone)]
pub struct PrepareResult {
    pub source: PathBuf,
    pub output: Option<PathBuf>,
    pub level: Level,
    pub outcome: Outcome,
    pub reasons: Vec<String>,
    pub tags_only: bool,
    pub report: Option<VerifyReport>,
    pub source_removed: bool,
    pub source_kept: Option<String>,
    pub error: Option<String>,
    pub error_kind: Option<ErrorKind>,
}

impl PrepareResult {
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.error_kind.is_none()
    }
}

/// A step ended the run without a mapped error of its own (no output, blockers).
#[derive(Debug, Clone)]
pub struct Failed {
    pub kind: ErrorKind,
    pub message: String,
    pub report: Option<VerifyReport>,
    pub output: Option<PathBuf>,
}

impl fmt::Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failed {}

/// Fail when `tmp` did not pass verification, after quarantining it as `name` when that is allowed.
///
/// `name` is the file `tmp` would have become. Only a standalone run (`quarantine`: the pipeline
/// pinned the source itself) may move files to a quarantine directory; a temp it cannot quarantine
/// is removed by the caller's cleanup.
///
/// # Errors
///
/// [`ErrorKind::Blockers`] when the report holds blockers.
pub fn settle_report(
    ctx: &mut RunContext,
    name: &str,
    options: &PrepareOptions,
    tmp: &Path,
    report: &VerifyReport,
    quarantine: bool,
) -> Result<(), Failed> {
    if report.passed() {
        return Ok(());
    }
    if quarantine && let Some(dir) = &options.quarantine_dir {
        // A quarantine that cannot be written must not hide the blockers.
        let _ = quarantine_output(&mut ctx.console, tmp, dir, name, report);
    }
    Err(Failed {
        kind: ErrorKind::Blockers,
        message: format!("compliance blockers in the output for {name}"),
        report: Some(report.clone()),
        output: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_to_crf_18_and_nothing_else() {
        let o = PrepareOptions::default();
        assert_eq!(o.crf, 18);
        assert!(!(o.force || o.dry_run || o.delete_original || o.copy_only || o.fix_dts));
        assert!(o.output.is_none() && o.quarantine_dir.is_none() && o.extra_sidecars.is_empty());
    }

    #[test]
    fn error_kinds_use_the_python_tags() {
        let tags: Vec<_> = [
            ErrorKind::Analysis,
            ErrorKind::Ffmpeg,
            ErrorKind::NoOutput,
            ErrorKind::Blockers,
            ErrorKind::Unsafe,
            ErrorKind::Transcode,
        ]
        .iter()
        .map(|k| k.as_str())
        .collect();
        assert_eq!(
            tags,
            [
                "analysis",
                "ffmpeg",
                "no_output",
                "blockers",
                "unsafe",
                "transcode"
            ]
        );
    }
}

//! Compliance rules for `AVPlayer`-strict MP4 files (port of `atvprep/compliance.py`).
//!
//! The rules catch production bugs observed on tvOS 26 when `AVPlayer` is fed files that `FFmpeg`
//! technically produces but `AVPlayer` mishandles. Blockers quarantine a file; warnings only show
//! up in the verify output.

pub mod atoms;
pub mod parse;
pub mod rules;
pub mod tools;

pub use rules::{
    Rule, RuleError, RuleResult, VideoReencodeRule, all_rules, rules_for, tag_findings,
};
pub use tools::{Audit, SystemTools, ToolError, Tools};

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The file would misbehave on tvOS; prepare quarantines it.
    Blocker,
    /// Worth knowing; does not stop anything.
    Warning,
}

impl Severity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blocker => "blocker",
            Self::Warning => "warning",
        }
    }
}

/// One finding of one rule.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Violation {
    pub rule: String,
    pub severity: Severity,
    pub message: String,
    pub track_id: Option<u32>,
}

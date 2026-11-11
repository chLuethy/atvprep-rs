//! The limits a schedule must respect, in one place.
//!
//! A schedule that changes files runs no more often than every [`MIN_CHANGE_INTERVAL_MINUTES`], and
//! each action caps its files per run to what one run can finish. The API enforces them on save,
//! the dispatcher pauses a stored schedule that exceeds them, and the UI reads them from the server.

use atvprep_core::encode::EncodeAllowance;

use crate::cron::CronExpr;

/// The shortest gap between two runs of a schedule that prepares files.
pub const MIN_CHANGE_INTERVAL_MINUTES: u32 = 15;
/// Files per run: checking is quick.
pub const MAX_FILES_VERIFY: i64 = 500;
/// Files per run: preparing without the video in the Kodierfreigabe, 5 to 10 minutes per file.
pub const MAX_FILES_PREPARE: i64 = 12;
/// Files per run: preparing with the video in the Kodierfreigabe may re-encode, which takes longest.
pub const MAX_FILES_PREPARE_VIDEO: i64 = 6;

/// The most files one run of this action may take under its Kodierfreigabe, or `None` for an
/// action without the field (scan). Only preparing reads `encode`.
#[must_use]
pub fn max_files_limit(action: &str, encode: EncodeAllowance) -> Option<i64> {
    match action {
        "verify" => Some(MAX_FILES_VERIFY),
        "prepare" if encode == EncodeAllowance::All => Some(MAX_FILES_PREPARE_VIDEO),
        "prepare" => Some(MAX_FILES_PREPARE),
        _ => None,
    }
}

/// Whether the action changes files and so has a minimum interval.
#[must_use]
pub fn changes_files(action: &str) -> bool {
    action == "prepare"
}

/// A schedule's settings that break a limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// Two runs are closer than the minimum interval.
    Interval { found: u32 },
    /// More files per run than the action allows.
    MaxFiles { limit: i64 },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interval { found } => write!(
                f,
                "A schedule that changes files may run at most every {MIN_CHANGE_INTERVAL_MINUTES} minutes; this one runs every {found}."
            ),
            Self::MaxFiles { limit } => {
                write!(f, "Files per run may be at most {limit} for this action.")
            }
        }
    }
}

/// Check the timing of a schedule against its action.
///
/// # Errors
///
/// [`Violation::Interval`] when a schedule that changes files runs more often than allowed.
pub fn check_timing(action: &str, cron: &CronExpr) -> Result<(), Violation> {
    match cron.min_interval_minutes() {
        Some(found) if changes_files(action) && found < MIN_CHANGE_INTERVAL_MINUTES => {
            Err(Violation::Interval { found })
        }
        _ => Ok(()),
    }
}

/// Check the files per run of a schedule against its action and Kodierfreigabe.
///
/// # Errors
///
/// [`Violation::MaxFiles`] when `max_files` is above the limit.
pub fn check_max_files(
    action: &str,
    encode: EncodeAllowance,
    max_files: i64,
) -> Result<(), Violation> {
    match max_files_limit(action, encode) {
        Some(limit) if max_files > limit => Err(Violation::MaxFiles { limit }),
        _ => Ok(()),
    }
}

/// Check a whole schedule: timing first, then files per run.
///
/// # Errors
///
/// The first [`Violation`] found.
pub fn check(
    action: &str,
    encode: EncodeAllowance,
    cron: &CronExpr,
    max_files: i64,
) -> Result<(), Violation> {
    check_timing(action, cron)?;
    check_max_files(action, encode, max_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atvprep_core::encode::EncodeAllowance::{All, Audio, Copy};

    fn cron(text: &str) -> CronExpr {
        CronExpr::parse(text).unwrap()
    }

    #[test]
    fn preparing_needs_fifteen_minutes() {
        assert!(check_timing("prepare", &cron("*/15 * * * *")).is_ok());
        assert_eq!(
            check_timing("prepare", &cron("0,10 * * * *")),
            Err(Violation::Interval { found: 10 })
        );
        assert_eq!(
            check_timing("prepare", &cron("* * * * *")),
            Err(Violation::Interval { found: 1 })
        );
        for action in ["verify", "scan"] {
            assert!(check_timing(action, &cron("* * * * *")).is_ok());
        }
    }

    #[test]
    fn max_files_is_capped_per_action_and_allowance() {
        assert!(check_max_files("verify", Copy, 500).is_ok());
        assert_eq!(
            check_max_files("verify", Copy, 501),
            Err(Violation::MaxFiles { limit: 500 })
        );
        for encode in [Copy, Audio] {
            assert!(check_max_files("prepare", encode, 12).is_ok());
            assert_eq!(
                check_max_files("prepare", encode, 13),
                Err(Violation::MaxFiles { limit: 12 })
            );
        }
        assert!(check_max_files("prepare", All, 6).is_ok());
        assert_eq!(
            check_max_files("prepare", All, 7),
            Err(Violation::MaxFiles { limit: 6 })
        );
        assert!(check_max_files("scan", Copy, 100_000).is_ok());
        assert!(Violation::MaxFiles { limit: 12 }.to_string().contains("12"));
    }

    #[test]
    fn the_old_actions_no_longer_change_files() {
        assert!(!changes_files("fix"));
        assert!(!changes_files("convert"));
        assert!(changes_files("prepare"));
    }
}

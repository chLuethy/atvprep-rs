//! Whether a schedule is due, as a pure decision.
//!
//! The dispatcher reads the schedule rows, asks [`decide`] and carries out the answer: it claims
//! the slot, runs or notes it and writes the history. Nothing here touches the database.

use chrono::{DateTime, TimeZone, Utc};

use crate::clock::format_micros;
use crate::cron::CronExpr;
use crate::schedules::{local_of, local_to_utc, next_cron_run_at};

/// How long after its slot a schedule that does not change files still runs.
pub const CATCH_UP_MICROS: i64 = 6 * 60 * 60 * 1_000_000;
/// A schedule that changes files runs only in the minute of its slot (plus a little slack for the
/// worker tick); a slot the worker missed, for any reason, is never caught up.
pub const ON_TIME_MICROS: i64 = 2 * 60 * 1_000_000;

/// What the decision depends on.
pub struct Due<'a, Tz: TimeZone> {
    /// The present moment.
    pub now: DateTime<Utc>,
    /// The timezone the cron is read in (the server's local time).
    pub timezone: &'a Tz,
    pub cron: &'a CronExpr,
    /// When the schedule was last turned on; a slot before it never ran and never will.
    pub armed_at: &'a str,
    /// The latest slot already claimed, as stored.
    pub last_slot_at: Option<&'a str>,
    /// Whether the schedule changes files (a narrow window, no catch-up).
    pub changes_files: bool,
    /// Whether the deployment gate lets anything run.
    pub gate_open: bool,
}

/// What the dispatcher has to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run the slot now.
    Run { slot: String },
    /// The slot left its window unrun: note it in the history, run nothing.
    Missed { slot: String, window_micros: i64 },
    /// The slot left its window behind a closed gate: use it up, report nothing.
    Lapsed { slot: String },
    /// Nothing to do; look again at `next_check` (the next slot, when there is one).
    Wait { next_check: Option<DateTime<Utc>> },
}

/// Decide what to do with a schedule right now.
#[must_use]
pub fn decide<Tz>(due: &Due<'_, Tz>) -> Decision
where
    Tz: TimeZone,
    Tz::Offset: chrono::Offset,
{
    let wait = || Decision::Wait {
        next_check: next_cron_run_at(due.now, true, due.cron, due.timezone),
    };
    let Some(local) = local_of(due.now, due.timezone) else {
        return wait();
    };
    let Some(slot_utc) = due
        .cron
        .latest_slot(local)
        .and_then(|slot| local_to_utc(slot, due.timezone))
    else {
        return wait();
    };
    let now_micros = due.now.timestamp_micros();
    let slot_micros = slot_utc.timestamp_micros();
    let slot = format_micros(slot_micros);
    // A slot before arming never ran and never will; one in the future is not due yet; one already
    // claimed is done.
    if slot_micros > now_micros
        || slot.as_str() <= due.armed_at
        || due.last_slot_at.is_some_and(|last| last >= slot.as_str())
    {
        return wait();
    }
    let window_micros = if due.changes_files {
        ON_TIME_MICROS
    } else {
        CATCH_UP_MICROS
    };
    let missed = now_micros - slot_micros > window_micros;
    match (due.gate_open, missed) {
        // Nothing runs behind a closed gate, and that is expected: a slot still inside its window
        // stays open for the catch-up, one that has left it is used up silently.
        (false, false) => wait(),
        (false, true) => Decision::Lapsed { slot },
        (true, false) => Decision::Run { slot },
        (true, true) => Decision::Missed {
            slot,
            window_micros,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Zurich;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    fn slot_text(at: DateTime<Utc>) -> String {
        format_micros(at.timestamp_micros())
    }

    struct Case {
        cron: &'static str,
        now: DateTime<Utc>,
        armed_at: &'static str,
        last_slot_at: Option<String>,
        changes_files: bool,
        gate_open: bool,
    }

    impl Case {
        fn new(cron: &'static str, now: DateTime<Utc>) -> Self {
            Self {
                cron,
                now,
                armed_at: "2000-01-01",
                last_slot_at: None,
                changes_files: false,
                gate_open: true,
            }
        }
        fn decide(&self) -> Decision {
            let cron = CronExpr::parse(self.cron).unwrap();
            decide(&Due {
                now: self.now,
                timezone: &Zurich,
                cron: &cron,
                armed_at: self.armed_at,
                last_slot_at: self.last_slot_at.as_deref(),
                changes_files: self.changes_files,
                gate_open: self.gate_open,
            })
        }
    }

    // 03:00 local in Zurich on 2026-10-20 (CEST, UTC+2) is 01:00 UTC.
    const DAILY_3: &str = "0 3 * * *";

    #[test]
    fn a_slot_inside_its_window_runs() {
        let case = Case::new(DAILY_3, utc(2026, 10, 20, 1, 1));
        assert_eq!(
            case.decide(),
            Decision::Run {
                slot: slot_text(utc(2026, 10, 20, 1, 0))
            }
        );
    }

    #[test]
    fn a_slot_in_the_future_waits_for_the_next_slot() {
        let case = Case::new(DAILY_3, utc(2026, 10, 20, 0, 59));
        // The latest slot is yesterday's, which is long past its window and already ran on time.
        let mut case = case;
        case.last_slot_at = Some(slot_text(utc(2026, 10, 19, 1, 0)));
        assert_eq!(
            case.decide(),
            Decision::Wait {
                next_check: Some(utc(2026, 10, 20, 1, 0))
            }
        );
    }

    #[test]
    fn a_claimed_slot_is_not_run_again() {
        let mut case = Case::new(DAILY_3, utc(2026, 10, 20, 1, 1));
        case.last_slot_at = Some(slot_text(utc(2026, 10, 20, 1, 0)));
        assert_eq!(
            case.decide(),
            Decision::Wait {
                next_check: Some(utc(2026, 10, 21, 1, 0))
            }
        );
    }

    #[test]
    fn a_slot_before_arming_never_runs() {
        let mut case = Case::new(DAILY_3, utc(2026, 10, 20, 1, 1));
        case.armed_at = "2026-10-20 01:00:30.000000";
        assert!(matches!(case.decide(), Decision::Wait { .. }));
    }

    #[test]
    fn a_slot_that_changes_files_is_missed_after_two_minutes() {
        let mut case = Case::new(DAILY_3, utc(2026, 10, 20, 1, 2));
        case.changes_files = true;
        assert!(matches!(case.decide(), Decision::Run { .. }));
        case.now = utc(2026, 10, 20, 1, 3);
        assert_eq!(
            case.decide(),
            Decision::Missed {
                slot: slot_text(utc(2026, 10, 20, 1, 0)),
                window_micros: ON_TIME_MICROS
            }
        );
    }

    #[test]
    fn other_slots_are_caught_up_for_six_hours() {
        let mut case = Case::new(DAILY_3, utc(2026, 10, 20, 7, 0));
        assert!(matches!(case.decide(), Decision::Run { .. }));
        case.now = utc(2026, 10, 20, 7, 1);
        assert!(matches!(
            case.decide(),
            Decision::Missed {
                window_micros: CATCH_UP_MICROS,
                ..
            }
        ));
    }

    #[test]
    fn a_closed_gate_keeps_a_slot_open_inside_its_window_and_uses_it_up_after() {
        let mut case = Case::new(DAILY_3, utc(2026, 10, 20, 1, 1));
        case.gate_open = false;
        assert!(matches!(case.decide(), Decision::Wait { .. }));
        case.now = utc(2026, 10, 20, 7, 1);
        assert_eq!(
            case.decide(),
            Decision::Lapsed {
                slot: slot_text(utc(2026, 10, 20, 1, 0))
            }
        );
    }

    #[test]
    fn the_slot_on_the_day_summer_time_ends_runs_once_at_its_first_occurrence() {
        // 02:30 local happens twice on 2026-10-25; the first is 00:30 UTC.
        let mut case = Case::new("30 2 * * *", utc(2026, 10, 25, 0, 31));
        assert_eq!(
            case.decide(),
            Decision::Run {
                slot: slot_text(utc(2026, 10, 25, 0, 30))
            }
        );
        // An hour later the clock reads 02:31 again; the slot is claimed and does not fire twice.
        case.last_slot_at = Some(slot_text(utc(2026, 10, 25, 0, 30)));
        case.now = utc(2026, 10, 25, 1, 31);
        assert!(matches!(case.decide(), Decision::Wait { .. }));
    }

    #[test]
    fn the_slot_inside_the_spring_gap_runs_at_the_offset_before_the_gap() {
        // 02:30 local does not exist on 2026-03-29; it counts as 01:30 UTC.
        let case = Case::new("30 2 * * *", utc(2026, 3, 29, 1, 31));
        assert_eq!(
            case.decide(),
            Decision::Run {
                slot: slot_text(utc(2026, 3, 29, 1, 30))
            }
        );
    }
}

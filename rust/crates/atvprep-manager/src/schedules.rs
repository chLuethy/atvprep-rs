//! Running schedules: the dispatcher that queues what is due and the run of one schedule. The
//! calendar (wall-clock time, DST, next slot) lives in [`crate::calendar`] and is re-exported here.

use chrono::{DateTime, Local, Utc};

use atvprep_core::encode::EncodeAllowance;

use crate::approval::ScheduleParams;
pub use crate::calendar::{
    LocalDateTime, TimeOfDay, days_to_mask, effective_cron, local_of, local_to_utc, mask_to_days,
    next_cron_run_at, next_cron_run_at_system, upcoming_cron_runs_system,
};
use crate::deployment_gate::DeploymentGate;
use crate::jobs::{Jobs, NewJob};
use crate::limits;
use crate::repo::Ctx;
use crate::scans::Scans;
use crate::schedule_candidates::{ScheduleSelection, candidate_ids};
use crate::schedule_due::{self, Decision};
use crate::settings::TargetProfile;
use crate::state_machine::JobAction;
use crate::{Error, Result};

/// Run one schedule or record why it could not queue work.
///
/// What a run does depends on the schedule's action: `verify` queues a verification job, `scan`
/// requests scans, and `fix`/`convert` queue a media-changing job only while the schedule carries
/// the operator's approval (otherwise nothing is queued and the result says so). The worker
/// verifies the signature again right before it touches a file (see ADR 0006).
///
/// # Errors
///
/// The gate is closed, the schedule does not exist, or SQLite rejects a read/write.
pub fn run_schedule(ctx: Ctx<'_>, schedule_id: i64) -> Result<String> {
    run_recorded(ctx, schedule_id, None)
}

/// One run of a schedule; `slot` is set when the dispatcher runs it for a slot, `None` for a manual run.
fn run_recorded(ctx: Ctx<'_>, schedule_id: i64, slot: Option<&str>) -> Result<String> {
    let gate = DeploymentGate(ctx).status()?;
    if !gate.healthy || gate.mode != "open" || gate.phase != "steady" {
        return Err(Error::Conflict("the deployment gate is closed".to_owned()));
    }
    let schedule: (String, Option<i64>, Option<String>, i64, String) = ctx.conn.query_row(
        "SELECT action, root_id, compliance_state, max_files, time_of_day FROM schedules WHERE id=?1",
        [schedule_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )?;
    let (action, root_id, compliance_state, max_files, _) = schedule;
    // A stored name no action has is treated as verifying, as it always was.
    let action = JobAction::from_public(&action).unwrap_or(JobAction::Verify);
    let now = ctx.clock.now();
    if action == JobAction::Scan {
        return run_scan_schedule(ctx, schedule_id, root_id, &now, slot);
    }
    if action.changes_files() {
        return run_change_schedule(ctx, schedule_id, &now, slot);
    }
    let media_ids = candidate_ids(
        ctx.conn,
        &ScheduleSelection {
            action,
            root_id,
            compliance_state,
            encode: EncodeAllowance::Copy,
        },
        Some(max_files),
    )?;
    if media_ids.is_empty() {
        let message = "No matching files";
        finish_run(ctx, schedule_id, slot, &now, message, None, None)?;
        return Ok(message.to_owned());
    }
    let mut job = NewJob::new(JobAction::Verify, &media_ids);
    job.trigger = "manual";
    job.settings_snapshot =
        serde_json::json!({"manual_approval": false, "schedule_id": schedule_id});
    let queued = Jobs(ctx).create(&job)?;
    let message = format!(
        "Queued {} {}",
        media_ids.len(),
        if media_ids.len() == 1 {
            "file"
        } else {
            "files"
        }
    );
    finish_run(
        ctx,
        schedule_id,
        slot,
        &now,
        &message,
        Some(queued.id),
        None,
    )?;
    Ok(message)
}

/// The `delete_original` runtime setting (off unless it is stored as `true`).
///
/// # Errors
///
/// SQLite rejects the read.
pub fn delete_original_setting(conn: &rusqlite::Connection) -> Result<bool> {
    let value: Option<String> = rusqlite::OptionalExtension::optional(conn.query_row(
        "SELECT value_json FROM runtime_settings WHERE key='delete_original'",
        [],
        |row| row.get(0),
    ))?;
    Ok(value.and_then(|text| serde_json::from_str::<bool>(&text).ok()) == Some(true))
}

/// A schedule row as far as the approval to change files is concerned.
pub struct ScheduleApproval {
    pub id: i64,
    pub action: String,
    pub root_id: Option<i64>,
    pub compliance_state: Option<String>,
    pub max_files: i64,
    /// The schedule's Kodierfreigabe as stored; an unknown name never matches a signature.
    pub encode: EncodeAllowance,
    pub enabled: bool,
    pub approved_by: Option<i64>,
    pub approved_at: Option<i64>,
    pub approval_mac: Option<String>,
}

impl ScheduleApproval {
    /// Load a schedule's current parameters and stored approval.
    ///
    /// # Errors
    ///
    /// SQLite rejects the read.
    pub fn load(conn: &rusqlite::Connection, id: i64) -> Result<Option<Self>> {
        Ok(rusqlite::OptionalExtension::optional(conn.query_row(
            "SELECT id, action, root_id, compliance_state, max_files, enabled, approved_by, approved_at, approval_mac, encode
             FROM schedules WHERE id=?1",
            [id],
            |row| {
                Ok(Self {
                    id: row.get(0)?,
                    action: row.get(1)?,
                    root_id: row.get(2)?,
                    compliance_state: row.get(3)?,
                    max_files: row.get(4)?,
                    enabled: row.get(5)?,
                    approved_by: row.get(6)?,
                    approved_at: row.get(7)?,
                    approval_mac: row.get(8)?,
                    encode: EncodeAllowance::parse(&row.get::<_, String>(9)?)
                        .unwrap_or(EncodeAllowance::Copy),
                })
            },
        ))?)
    }

    /// The parameters an approval must match, with the given `delete_original` setting and
    /// Zielprofil.
    #[must_use]
    pub fn params(&self, delete_original: bool, profile: TargetProfile) -> ScheduleParams<'_> {
        ScheduleParams {
            schedule_id: self.id,
            action: &self.action,
            root_id: self.root_id,
            compliance_state: self.compliance_state.as_deref(),
            max_files: self.max_files,
            delete_original,
            encode: self.encode,
            profile,
        }
    }
}

/// Queue the files a schedule that prepares selects, if the operator approved it.
///
/// Only files whose Kodierbedarf lies within the schedule's Kodierfreigabe are taken, and every
/// item carries that Kodierfreigabe. The signature itself is verified by the worker right before a
/// file is touched (it needs the secret); here an approval merely has to exist, so an unapproved
/// schedule queues nothing.
fn run_change_schedule(
    ctx: Ctx<'_>,
    schedule_id: i64,
    now: &str,
    slot: Option<&str>,
) -> Result<String> {
    let finish = |message: String, job: Option<i64>| -> Result<String> {
        finish_run(ctx, schedule_id, slot, now, &message, job, None)?;
        Ok(message)
    };
    let Some(row) = ScheduleApproval::load(ctx.conn, schedule_id)? else {
        return Err(Error::Invalid("schedule not found".to_owned()));
    };
    if row.approval_mac.is_none() || row.approved_by.is_none() || row.approved_at.is_none() {
        return finish(
            "Not queued: not approved to change files. Turn the schedule on again.".to_owned(),
            None,
        );
    }
    let delete_original = delete_original_setting(ctx.conn)?;
    let profile = crate::settings::target_profile(ctx.conn)?;
    let media_ids = candidate_ids(
        ctx.conn,
        &ScheduleSelection {
            action: JobAction::Prepare,
            root_id: row.root_id,
            compliance_state: row.compliance_state.clone(),
            encode: row.encode,
        },
        Some(row.max_files),
    )?;
    if media_ids.is_empty() {
        return finish("No matching files".to_owned(), None);
    }
    let params = row.params(delete_original, profile);
    let mut job = NewJob::new(JobAction::Prepare, &media_ids);
    job.trigger = "manual";
    // Every item carries the signed Kodierfreigabe; the worker refuses one that differs.
    job.encode_media = media_ids.iter().map(|id| (*id, row.encode)).collect();
    job.settings_snapshot = crate::prepare_snapshot::PrepareSnapshot {
        profile,
        delete_original,
        delete_original_confirmed: false,
        encode: None,
        force: false,
        planned: None,
        origin: crate::prepare_snapshot::Origin::Schedule {
            schedule_id,
            params: params.to_json(),
        },
    }
    .to_json();
    let queued = Jobs(ctx).create(&job)?;
    let count = media_ids.len();
    finish(
        format!(
            "Queued {count} {}",
            if count == 1 { "file" } else { "files" }
        ),
        Some(queued.id),
    )
}

/// Queue the scan(s) of a scan schedule: one library, or every library when none is chosen.
fn run_scan_schedule(
    ctx: Ctx<'_>,
    schedule_id: i64,
    root_id: Option<i64>,
    now: &str,
    slot: Option<&str>,
) -> Result<String> {
    let roots = if let Some(root) = root_id {
        vec![root]
    } else {
        let mut statement = ctx
            .conn
            .prepare("SELECT id FROM library_roots ORDER BY id")?;
        statement
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<i64>, _>>()?
    };
    let scans = Scans(ctx);
    let mut first_scan = None;
    for root in &roots {
        let scan = scans.request_manual(*root)?;
        first_scan.get_or_insert(scan.id);
    }
    let message = match roots.len() {
        0 => "No libraries".to_owned(),
        1 => "Queued 1 scan".to_owned(),
        count => format!("Queued {count} scans"),
    };
    finish_run(ctx, schedule_id, slot, now, &message, None, first_scan)?;
    Ok(message)
}

/// Remember the outcome on the schedule and add it to its run history.
fn finish_run(
    ctx: Ctx<'_>,
    schedule_id: i64,
    slot: Option<&str>,
    now: &str,
    message: &str,
    job_id: Option<i64>,
    scan_id: Option<i64>,
) -> Result<()> {
    ctx.conn.execute(
        "UPDATE schedules SET last_run_at=?1, last_job_id=?2, last_result=?3 WHERE id=?4",
        rusqlite::params![now, job_id, message, schedule_id],
    )?;
    ctx.conn.execute(
        "INSERT INTO schedule_runs(schedule_id, slot, started_at, trigger, result, job_id, scan_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            schedule_id,
            slot,
            now,
            if slot.is_some() {
                "scheduled"
            } else {
                "manual"
            },
            message,
            job_id,
            scan_id
        ],
    )?;
    Ok(())
}

/// Pause a schedule that breaks a limit, end its approval and put the reason into its history.
fn pause_over_limit(ctx: Ctx<'_>, schedule_id: i64, violation: &limits::Violation) -> Result<()> {
    ctx.conn.execute(
        "UPDATE schedules SET enabled=0, approved_by=NULL, approved_at=NULL, approval_mac=NULL WHERE id=?1",
        [schedule_id],
    )?;
    finish_run(
        ctx,
        schedule_id,
        None,
        &ctx.clock.now(),
        &format!("Paused: {violation}"),
        None,
        None,
    )
}

/// Note a slot the manager did not trigger within its window in the schedule's history, with no job.
///
/// The claim on the slot (done by the caller) keeps later ticks from noting it again; the unique
/// index on missed entries guarantees it even if two schedulers raced past the claim.
fn note_missed(ctx: Ctx<'_>, schedule_id: i64, slot: &str, window_micros: i64) -> Result<()> {
    ctx.conn.execute(
        "INSERT OR IGNORE INTO schedule_runs(schedule_id, slot, started_at, trigger, result, kind)
         VALUES (?1, ?2, ?3, 'scheduled', ?4, 'missed')",
        rusqlite::params![
            schedule_id,
            slot,
            ctx.clock.now(),
            format!(
                "Missed: not started within {} minutes of its slot",
                window_micros / 60_000_000
            )
        ],
    )?;
    Ok(())
}

/// Run enabled schedules whose latest local slot is no more than six hours old; schedules that
/// change files only run in the minute of their slot.
///
/// Each slot is claimed with a compare-and-set update, so concurrent schedulers run it once.
///
/// # Errors
///
/// SQLite rejects a schedule query or its claimed schedule cannot be run.
pub fn run_due_schedules(ctx: Ctx<'_>) -> Result<usize> {
    if !ctx.conn.is_autocommit() {
        return run_due_schedules_inner(ctx);
    }
    let tx =
        rusqlite::Transaction::new_unchecked(ctx.conn, rusqlite::TransactionBehavior::Immediate)?;
    let ran = run_due_schedules_inner(Ctx {
        conn: &tx,
        clock: ctx.clock,
    })?;
    tx.commit()?;
    Ok(ran)
}

fn run_due_schedules_inner(ctx: Ctx<'_>) -> Result<usize> {
    let gate = DeploymentGate(ctx).status()?;
    let gate_open = gate.healthy && gate.mode == "open" && gate.phase == "steady";
    let Some(now) = DateTime::<Utc>::from_timestamp_micros(ctx.clock.now_micros()) else {
        return Ok(0);
    };
    let mut statement = ctx.conn.prepare(
        "SELECT id, days_mask, time_of_day, armed_at, last_slot_at, cron, action, max_files, encode FROM schedules WHERE enabled=1 ORDER BY id",
    )?;
    let schedules = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, u8>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let mut ran = 0;
    for (id, mask, time, armed_at, last_slot_at, cron, action, max_files, encode) in schedules {
        let encode = EncodeAllowance::parse(&encode).unwrap_or(EncodeAllowance::All);
        // One corrupt row must not stop every schedule after it.
        let Ok(cron) = effective_cron(cron.as_deref(), mask, &time) else {
            continue;
        };
        // A schedule saved before a limit existed does not run on unnoticed: it is paused and says why.
        if let Err(violation) = limits::check(&action, encode, &cron, max_files)
            && gate_open
        {
            pause_over_limit(ctx, id, &violation)?;
            continue;
        }
        let decision = schedule_due::decide(&schedule_due::Due {
            now,
            timezone: &Local,
            cron: &cron,
            armed_at: &armed_at,
            last_slot_at: last_slot_at.as_deref(),
            changes_files: limits::changes_files(&action),
            gate_open,
        });
        let (slot_text, missed_window) = match decision {
            Decision::Wait { .. } => continue,
            Decision::Lapsed { slot } => {
                claim_slot(ctx, id, &slot)?;
                continue;
            }
            Decision::Run { slot } => (slot, None),
            Decision::Missed {
                slot,
                window_micros,
            } => (slot, Some(window_micros)),
        };
        // A failing schedule rolls back alone: its claim is retried next pass, the others still run.
        ctx.conn.execute_batch("SAVEPOINT schedule_run")?;
        let outcome = (|| -> Result<bool> {
            if !claim_slot(ctx, id, &slot_text)? {
                return Ok(false);
            }
            if let Some(window) = missed_window {
                note_missed(ctx, id, &slot_text, window)?;
                return Ok(false);
            }
            let _ = run_recorded(ctx, id, Some(&slot_text))?;
            Ok(true)
        })();
        match outcome {
            Ok(did_run) => {
                ctx.conn.execute_batch("RELEASE schedule_run")?;
                if did_run {
                    ran += 1;
                }
            }
            Err(_) => {
                ctx.conn
                    .execute_batch("ROLLBACK TO schedule_run; RELEASE schedule_run")?;
            }
        }
    }
    Ok(ran)
}

/// Claim a slot with a compare-and-set: only the dispatcher that moves `last_slot_at` forward runs it.
fn claim_slot(ctx: Ctx<'_>, schedule_id: i64, slot: &str) -> Result<bool> {
    Ok(ctx.conn.execute(
        "UPDATE schedules SET last_slot_at=?1 WHERE id=?2 AND (last_slot_at IS NULL OR last_slot_at < ?1)",
        rusqlite::params![slot, schedule_id],
    )? == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn local(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> LocalDateTime {
        LocalDateTime::new(year, month, day, hour, minute).unwrap()
    }
    fn time(value: &str) -> TimeOfDay {
        TimeOfDay::parse(value).unwrap()
    }

    fn runner_fixture() -> (
        crate::db::Db,
        crate::db::Conn,
        crate::clock::FixedClock,
        i64,
    ) {
        let database = crate::db::Db::memory().unwrap();
        let mut conn = database.conn().unwrap();
        crate::schema::migrate(&mut conn).unwrap();
        let clock = crate::clock::FixedClock(1_700_000_000_000_000);
        conn.raw().execute_batch(
            "INSERT INTO library_roots(name, canonical_path) VALUES ('r', '/r');
             INSERT INTO media_files(root_id, relative_path, kind, suffix, size, mtime_ns, fingerprint,
                 scanned_device, scanned_inode, scanned_ctime_ns, discovery_state, compliance_state,
                 workflow_state, desired_action)
             VALUES (1, 'a.mp4', 'video', '.mp4', 1, 1, 'a', 1, 1, 1, 'present', 'unknown', 'idle', 'none');
             INSERT INTO schedules(name, action, days_mask, time_of_day, max_files, enabled)
             VALUES ('verify', 'verify', 127, '00:00', 25, 0);",
        ).unwrap();
        (database, conn, clock, 1)
    }

    #[test]
    fn schedule_runner_queues_verify_and_refuses_unapproved_writes() {
        let (_database, conn, clock, schedule_id) = runner_fixture();
        let ctx = crate::repo::Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        assert_eq!(run_schedule(ctx, schedule_id).unwrap(), "Queued 1 file");
        let queued: (String, String) = conn
            .raw()
            .query_row("SELECT jobs.action, jobs.trigger FROM jobs", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(queued, ("verify".to_owned(), "manual".to_owned()));
        conn.raw()
            .execute(
                "UPDATE schedules SET action='prepare' WHERE id=?1",
                [schedule_id],
            )
            .unwrap();
        assert_eq!(
            run_schedule(ctx, schedule_id).unwrap(),
            "Not queued: not approved to change files. Turn the schedule on again."
        );
        let jobs: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM jobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(jobs, 1);
    }

    #[test]
    fn every_run_is_recorded_in_the_history() {
        let (_database, conn, clock, schedule_id) = runner_fixture();
        let ctx = crate::repo::Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        let runs = |conn: &crate::db::Conn| -> Vec<(Option<String>, String, String, bool, bool)> {
            conn.raw()
                .prepare(
                    "SELECT slot, trigger, result, job_id IS NOT NULL, scan_id IS NOT NULL
                     FROM schedule_runs ORDER BY id",
                )
                .unwrap()
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })
                .unwrap()
                .map(|row| row.unwrap())
                .collect()
        };
        run_schedule(ctx, schedule_id).unwrap();
        conn.raw()
            .execute("UPDATE media_files SET discovery_state='missing'", [])
            .unwrap();
        run_schedule(ctx, schedule_id).unwrap();
        conn.raw()
            .execute("UPDATE schedules SET action='prepare'", [])
            .unwrap();
        run_schedule(ctx, schedule_id).unwrap();
        conn.raw()
            .execute("UPDATE schedules SET action='scan'", [])
            .unwrap();
        run_schedule(ctx, schedule_id).unwrap();
        let manual = "manual".to_owned();
        assert_eq!(
            runs(&conn),
            vec![
                (
                    None,
                    manual.clone(),
                    "Queued 1 file".to_owned(),
                    true,
                    false
                ),
                (
                    None,
                    manual.clone(),
                    "No matching files".to_owned(),
                    false,
                    false
                ),
                (
                    None,
                    manual.clone(),
                    "Not queued: not approved to change files. Turn the schedule on again."
                        .to_owned(),
                    false,
                    false
                ),
                (None, manual, "Queued 1 scan".to_owned(), false, true),
            ]
        );
        let last: String = conn
            .raw()
            .query_row("SELECT last_result FROM schedules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(last, "Queued 1 scan");
    }

    #[test]
    fn schedule_runner_obeys_a_closed_gate() {
        let (_database, conn, clock, schedule_id) = runner_fixture();
        conn.raw()
            .execute(
                "UPDATE deployment_gate SET mode='maintenance' WHERE id=1",
                [],
            )
            .unwrap();
        let ctx = crate::repo::Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        assert!(run_schedule(ctx, schedule_id).is_err());
        let jobs: i64 = conn
            .raw()
            .query_row("SELECT count(*) FROM jobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(jobs, 0);
    }

    #[test]
    fn weekdays_start_on_monday_and_deduplicate() {
        assert_eq!(days_to_mask(&[6, 0, 3, 3]), 73);
        assert_eq!(mask_to_days(days_to_mask(&[6, 0, 3, 3])), vec![0, 3, 6]);
        assert_eq!(local(2026, 10, 26, 0, 0).weekday(), 0);
    }

    #[test]
    fn time_of_day_is_strict() {
        assert_eq!(time("03:04").to_string(), "03:04");
        for bad in ["3:04", "03:4", "24:00", "23:60", "nope"] {
            assert!(TimeOfDay::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn calendar_arithmetic_crosses_leap_day() {
        assert_eq!(
            local(2028, 2, 28, 12, 0).next_day(),
            local(2028, 2, 29, 12, 0)
        );
        assert_eq!(
            local(2028, 3, 1, 12, 0).previous_day(),
            local(2028, 2, 29, 12, 0)
        );
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn dst_conversion_matches_python_for_repeated_and_missing_times() {
        let timezone = chrono_tz::Europe::Zurich;
        assert_eq!(
            local_to_utc(local(2026, 10, 25, 2, 30), &timezone),
            Some(utc(2026, 10, 25, 0, 30))
        );
        assert_eq!(
            local_to_utc(local(2026, 3, 29, 2, 30), &timezone),
            Some(utc(2026, 3, 29, 1, 30))
        );
    }

    #[test]
    fn a_blank_cron_falls_back_to_the_legacy_columns() {
        assert_eq!(
            effective_cron(Some(" "), 1, "03:00").unwrap().as_str(),
            "0 3 * * 1"
        );
        assert!(effective_cron(None, 1, "bad").is_err());
        assert!(effective_cron(Some("nonsense"), 1, "03:00").is_err());
    }

    #[test]
    fn the_next_cron_run_is_in_the_future_across_dst_boundaries() {
        let timezone = chrono_tz::Europe::Zurich;
        let cron = effective_cron(None, 127, "02:30").unwrap();
        for (year, month, day) in [(2026, 3, 28), (2026, 3, 29), (2026, 10, 24), (2026, 10, 25)] {
            for hour in 0..24 {
                let now = utc(year, month, day, hour, 0);
                let following = next_cron_run_at(now, true, &cron, &timezone).unwrap();
                assert!(
                    following > now && following <= now + chrono::TimeDelta::hours(25),
                    "{now} -> {following}"
                );
            }
        }
        assert!(next_cron_run_at(utc(2026, 3, 29, 0, 0), false, &cron, &timezone).is_none());
    }
}

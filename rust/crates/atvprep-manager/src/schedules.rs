//! Pure local-calendar primitives for weekly schedules.
//!
//! The worker deliberately keeps a scheduled wall-clock time separate from its
//! UTC conversion.  The latter depends on the host timezone and DST policy;
//! these primitives are deterministic and use Monday as weekday zero, exactly
//! like Python's `datetime.weekday()`.

use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDate, Offset, TimeZone, Timelike, Utc};

use crate::approval::ScheduleParams;
use crate::clock::format_micros;
use crate::cron::CronExpr;
use crate::deployment_gate::DeploymentGate;
use crate::jobs::{Jobs, NewJob};
use crate::limits;
use crate::repo::Ctx;
use crate::scans::Scans;
use crate::{Error, Result};

const CATCH_UP_MICROS: i64 = 6 * 60 * 60 * 1_000_000;
/// A schedule that changes files runs only in the minute of its slot (plus a little slack for the
/// worker tick); a slot the worker missed, for any reason, is never caught up.
const ON_TIME_MICROS: i64 = 2 * 60 * 1_000_000;

/// A validated local wall-clock minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeOfDay {
    hour: u8,
    minute: u8,
}

impl TimeOfDay {
    /// Parse the strict `HH:MM` form stored in the database.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the value is not a real, zero-padded
    /// 24-hour clock time.
    pub fn parse(value: &str) -> Result<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != 5
            || bytes[2] != b':'
            || !bytes[0].is_ascii_digit()
            || !bytes[1].is_ascii_digit()
            || !bytes[3].is_ascii_digit()
            || !bytes[4].is_ascii_digit()
        {
            return Err(invalid_time());
        }
        let hour = (bytes[0] - b'0') * 10 + bytes[1] - b'0';
        let minute = (bytes[3] - b'0') * 10 + bytes[4] - b'0';
        if hour > 23 || minute > 59 {
            return Err(invalid_time());
        }
        Ok(Self { hour, minute })
    }

    #[must_use]
    pub const fn hour(self) -> u8 {
        self.hour
    }

    #[must_use]
    pub const fn minute(self) -> u8 {
        self.minute
    }
}

impl std::fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

fn invalid_time() -> Error {
    Error::Invalid("time_of_day must be a valid HH:MM time".to_owned())
}

/// A validated naive local date and minute, independent of a timezone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalDateTime {
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
}

impl LocalDateTime {
    /// Construct a local wall-clock value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for an invalid proleptic-Gregorian date or
    /// clock time.
    pub fn new(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> Result<Self> {
        if !(1..=12).contains(&month)
            || day == 0
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
        {
            return Err(Error::Invalid("invalid local date and time".to_owned()));
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
        })
    }

    #[must_use]
    pub const fn year(self) -> i32 {
        self.year
    }
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }
    #[must_use]
    pub const fn hour(self) -> u8 {
        self.hour
    }
    #[must_use]
    pub const fn minute(self) -> u8 {
        self.minute
    }

    #[must_use]
    pub fn weekday(self) -> u8 {
        // 1970-01-01 was Thursday (Monday = 0).
        let weekday = (days_from_civil(self.year, self.month, self.day) + 3).rem_euclid(7);
        u8::try_from(weekday).unwrap_or(0) // `rem_euclid(7)` is necessarily in 0..7.
    }

    #[must_use]
    pub fn with_time(self, time: TimeOfDay) -> Self {
        Self {
            hour: time.hour,
            minute: time.minute,
            ..self
        }
    }

    #[must_use]
    pub fn previous_day(self) -> Self {
        self.add_days(-1)
    }
    #[must_use]
    pub fn next_day(self) -> Self {
        self.add_days(1)
    }

    /// # Panics
    ///
    /// Panics only when an addition would move beyond the representable `i32`
    /// year range; normal schedules only add at most seven days.
    #[must_use]
    pub fn add_days(self, days: i64) -> Self {
        let (year, month, day) =
            civil_from_days(days_from_civil(self.year, self.month, self.day) + days);
        Self {
            year: i32::try_from(year).expect("schedule date year remains representable"),
            month: u8::try_from(month).expect("calendar month is in 1..=12"),
            day: u8::try_from(day).expect("calendar day is in 1..=31"),
            hour: self.hour,
            minute: self.minute,
        }
    }
}

/// Convert a possibly repeated list of Python weekday numbers to its bit mask.
#[must_use]
pub fn days_to_mask(days: &[u8]) -> u8 {
    days.iter().fold(
        0,
        |mask, &day| if day < 7 { mask | (1 << day) } else { mask },
    )
}

/// Return selected weekday numbers in Monday-to-Sunday order.
#[must_use]
pub fn mask_to_days(mask: u8) -> Vec<u8> {
    (0..7).filter(|day| mask & (1 << day) != 0).collect()
}

/// The latest selected local slot that is not later than `now`.
#[must_use]
pub fn latest_slot(now: LocalDateTime, days_mask: u8, time: TimeOfDay) -> Option<LocalDateTime> {
    for back in 0..8 {
        let day = now.add_days(-back);
        if days_mask & (1 << day.weekday()) != 0 {
            let slot = day.with_time(time);
            if slot <= now {
                return Some(slot);
            }
        }
    }
    None
}

/// The first selected local slot strictly later than `now`.
#[must_use]
pub fn next_slot(now: LocalDateTime, days_mask: u8, time: TimeOfDay) -> Option<LocalDateTime> {
    for ahead in 0..8 {
        let day = now.add_days(ahead);
        if days_mask & (1 << day.weekday()) != 0 {
            let slot = day.with_time(time);
            if slot > now {
                return Some(slot);
            }
        }
    }
    None
}

fn as_chrono(local: LocalDateTime) -> Option<chrono::NaiveDateTime> {
    NaiveDate::from_ymd_opt(local.year, u32::from(local.month), u32::from(local.day))?.and_hms_opt(
        u32::from(local.hour),
        u32::from(local.minute),
        0,
    )
}

/// Convert a local scheduled time to UTC using Python-compatible DST handling.
///
/// An ambiguous wall-clock time is its first occurrence. For a nonexistent
/// wall-clock time during a forward DST change, Python applies the offset in
/// effect immediately before the gap; this implementation does the same.
#[must_use]
pub fn local_to_utc<Tz>(local: LocalDateTime, timezone: &Tz) -> Option<DateTime<Utc>>
where
    Tz: TimeZone,
    Tz::Offset: Offset,
{
    let naive = as_chrono(local)?;
    match timezone.from_local_datetime(&naive) {
        LocalResult::Single(moment) => Some(moment.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
        LocalResult::None => {
            // The prior valid offset is Python's interpretation of an
            // otherwise nonexistent naive local datetime.
            for minutes_back in 1..=48 * 60 {
                let before = naive.checked_sub_signed(chrono::TimeDelta::minutes(minutes_back))?;
                match timezone.from_local_datetime(&before) {
                    LocalResult::Single(moment) | LocalResult::Ambiguous(moment, _) => {
                        let offset = moment.offset().fix();
                        return offset
                            .from_local_datetime(&naive)
                            .single()
                            .map(|moment| moment.with_timezone(&Utc));
                    }
                    LocalResult::None => {}
                }
            }
            None
        }
    }
}

/// Find the next future scheduled instant in an explicitly supplied timezone.
#[must_use]
pub fn next_run_at<Tz>(
    now: DateTime<Utc>,
    enabled: bool,
    days_mask: u8,
    time: TimeOfDay,
    timezone: &Tz,
) -> Option<DateTime<Utc>>
where
    Tz: TimeZone,
    Tz::Offset: Offset,
{
    if !enabled {
        return None;
    }
    let local_now = now.with_timezone(timezone);
    let mut slot = next_slot(
        LocalDateTime::new(
            local_now.year(),
            u8::try_from(local_now.month()).ok()?,
            u8::try_from(local_now.day()).ok()?,
            u8::try_from(local_now.hour()).ok()?,
            u8::try_from(local_now.minute()).ok()?,
        )
        .ok()?,
        days_mask,
        time,
    );
    while let Some(local_slot) = slot {
        let utc = local_to_utc(local_slot, timezone)?;
        if utc > now {
            return Some(utc);
        }
        slot = next_slot(local_slot, days_mask, time);
    }
    None
}

/// Find the next future scheduled instant in the manager host's local timezone.
#[must_use]
pub fn next_run_at_system(
    now: DateTime<Utc>,
    enabled: bool,
    days_mask: u8,
    time: TimeOfDay,
) -> Option<DateTime<Utc>> {
    next_run_at(now, enabled, days_mask, time, &Local)
}

/// The cron a schedule row runs on: its `cron` column, or, while that is empty (a row written
/// by the previous binary), the one its legacy weekday mask and time of day stand for.
///
/// # Errors
///
/// [`Error::Invalid`] when the stored cron or the legacy time is malformed.
pub fn effective_cron(cron: Option<&str>, days_mask: u8, time_of_day: &str) -> Result<CronExpr> {
    match cron.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => CronExpr::parse(text),
        None => Ok(CronExpr::from_weekdays(
            days_mask,
            TimeOfDay::parse(time_of_day)?,
        )),
    }
}

/// Find the next future cron slot as a UTC instant in an explicitly supplied timezone.
///
/// DST is handled exactly like the weekly slots: see [`local_to_utc`].
#[must_use]
pub fn next_cron_run_at<Tz>(
    now: DateTime<Utc>,
    enabled: bool,
    cron: &CronExpr,
    timezone: &Tz,
) -> Option<DateTime<Utc>>
where
    Tz: TimeZone,
    Tz::Offset: Offset,
{
    if !enabled {
        return None;
    }
    let local_now = now.with_timezone(timezone);
    let mut slot = cron.next_slot(
        LocalDateTime::new(
            local_now.year(),
            u8::try_from(local_now.month()).ok()?,
            u8::try_from(local_now.day()).ok()?,
            u8::try_from(local_now.hour()).ok()?,
            u8::try_from(local_now.minute()).ok()?,
        )
        .ok()?,
    );
    while let Some(local_slot) = slot {
        let utc = local_to_utc(local_slot, timezone)?;
        if utc > now {
            return Some(utc);
        }
        slot = cron.next_slot(local_slot);
    }
    None
}

/// [`next_cron_run_at`] in the manager host's local timezone.
#[must_use]
pub fn next_cron_run_at_system(
    now: DateTime<Utc>,
    enabled: bool,
    cron: &CronExpr,
) -> Option<DateTime<Utc>> {
    next_cron_run_at(now, enabled, cron, &Local)
}

/// The next `count` future cron instants after `now`, in the manager host's local timezone.
#[must_use]
pub fn upcoming_cron_runs_system(
    now: DateTime<Utc>,
    cron: &CronExpr,
    count: usize,
) -> Vec<DateTime<Utc>> {
    let mut runs = Vec::with_capacity(count);
    let mut after = now;
    while runs.len() < count {
        let Some(next) = next_cron_run_at_system(after, true, cron) else {
            break;
        };
        runs.push(next);
        after = next;
    }
    runs
}

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
    let now = ctx.clock.now();
    if action == "scan" {
        return run_scan_schedule(ctx, schedule_id, root_id, &now, slot);
    }
    if matches!(action.as_str(), "fix" | "convert") {
        return run_change_schedule(ctx, schedule_id, &action, &now, slot);
    }
    let mut statement = ctx.conn.prepare(
        "SELECT media_files.id FROM media_files
         WHERE discovery_state='present'
           AND lower(suffix) IN ('.mp4','.m4v','.mov')
           AND (?1 IS NULL OR root_id=?1)
           AND (?2 IS NULL OR compliance_state=?2)
           AND id NOT IN (SELECT media_file_id FROM job_items
                          WHERE action IN ('fix','convert')
                            AND status NOT IN ('succeeded','failed','cancelled','interrupted','stale_input'))
         ORDER BY (SELECT max(id) FROM job_items AS prior
                   WHERE prior.media_file_id=media_files.id AND prior.action='verify') ASC,
                  media_files.id ASC LIMIT ?3",
    )?;
    let media_ids = statement
        .query_map(
            rusqlite::params![root_id, compliance_state, max_files],
            |row| row.get(0),
        )?
        .collect::<std::result::Result<Vec<i64>, _>>()?;
    drop(statement);
    if media_ids.is_empty() {
        let message = "No matching files";
        finish_run(ctx, schedule_id, slot, &now, message, None, None)?;
        return Ok(message.to_owned());
    }
    let mut job = NewJob::new("verify", &media_ids);
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
            "SELECT id, action, root_id, compliance_state, max_files, enabled, approved_by, approved_at, approval_mac
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
                })
            },
        ))?)
    }

    /// The parameters an approval must match, with the given `delete_original` setting.
    #[must_use]
    pub fn params(&self, delete_original: bool) -> ScheduleParams<'_> {
        ScheduleParams {
            schedule_id: self.id,
            action: &self.action,
            root_id: self.root_id,
            compliance_state: self.compliance_state.as_deref(),
            max_files: self.max_files,
            delete_original,
        }
    }
}

/// Queue the files a repair or conversion schedule selects, if the operator approved it.
///
/// The signature itself is verified by the worker right before a file is touched (it needs the
/// secret); here an approval merely has to exist, so an unapproved schedule queues nothing.
fn run_change_schedule(
    ctx: Ctx<'_>,
    schedule_id: i64,
    action: &str,
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
    let mut statement = ctx.conn.prepare(
        "SELECT m.id FROM media_files m
         WHERE m.discovery_state='present'
           AND (?1 IS NULL OR m.root_id=?1)
           AND (?2 IS NULL OR m.compliance_state=?2)
           AND NOT EXISTS (SELECT 1 FROM job_items i WHERE i.media_file_id=m.id
                           AND i.action IN ('fix','convert')
                           AND i.status NOT IN ('succeeded','failed','cancelled','interrupted','stale_input'))
           AND NOT EXISTS (SELECT 1 FROM media_files sibling
                           WHERE sibling.root_id=m.root_id AND sibling.discovery_state='present'
                             AND sibling.relative_path=substr(m.relative_path,1,length(m.relative_path)-length(m.suffix))||'.mp4'
                             AND lower(m.suffix) NOT IN ('.mp4','.m4v','.mov'))
           AND (lower(m.suffix) NOT IN ('.mp4','.m4v','.mov')
                OR m.compliance_state IN ('warning','blocker') OR m.desired_action='fix')
           AND (?3 OR m.compliance_reasons NOT LIKE '%] video-reencode:%')
         ORDER BY m.id LIMIT ?4",
    )?;
    let media_ids = statement
        .query_map(
            rusqlite::params![
                row.root_id,
                row.compliance_state,
                delete_original,
                row.max_files
            ],
            |r| r.get(0),
        )?
        .collect::<std::result::Result<Vec<i64>, _>>()?;
    drop(statement);
    if media_ids.is_empty() {
        return finish("No matching files".to_owned(), None);
    }
    let params = row.params(delete_original);
    let mut job = NewJob::new(
        if action == "convert" {
            "convert"
        } else {
            "fix"
        },
        &media_ids,
    );
    job.trigger = "manual";
    job.settings_snapshot = serde_json::json!({
        "pipeline": "prepare",
        "policy": "copy-only-v1",
        "delete_original": delete_original,
        "delete_original_confirmed": false,
        "manual_approval": false,
        "schedule_id": schedule_id,
        "schedule_params": params.to_json(),
    });
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
    if !gate.healthy || gate.mode != "open" || gate.phase != "steady" {
        return Ok(0);
    }
    let now_micros = ctx.clock.now_micros();
    let Some(now_utc) = DateTime::<Utc>::from_timestamp_micros(now_micros) else {
        return Ok(0);
    };
    let local_now = now_utc.with_timezone(&Local);
    let local = LocalDateTime::new(
        local_now.year(),
        u8::try_from(local_now.month()).unwrap_or(0),
        u8::try_from(local_now.day()).unwrap_or(0),
        u8::try_from(local_now.hour()).unwrap_or(0),
        u8::try_from(local_now.minute()).unwrap_or(0),
    )?;
    let mut statement = ctx.conn.prepare(
        "SELECT id, days_mask, time_of_day, armed_at, cron, action, max_files FROM schedules WHERE enabled=1 ORDER BY id",
    )?;
    let schedules = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, u8>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let mut ran = 0;
    for (id, mask, time, armed_at, cron, action, max_files) in schedules {
        // One corrupt row must not stop every schedule after it.
        let Ok(cron) = effective_cron(cron.as_deref(), mask, &time) else {
            continue;
        };
        // A schedule saved before a limit existed does not run on unnoticed: it is paused and says why.
        if let Err(violation) = limits::check(&action, &cron, max_files) {
            pause_over_limit(ctx, id, &violation)?;
            continue;
        }
        let Some(slot) = cron.latest_slot(local) else {
            continue;
        };
        let Some(slot_utc) = local_to_utc(slot, &Local) else {
            continue;
        };
        let slot_micros = slot_utc.timestamp_micros();
        if slot_micros > now_micros
            || now_micros - slot_micros
                > if matches!(action.as_str(), "fix" | "convert") {
                    ON_TIME_MICROS
                } else {
                    CATCH_UP_MICROS
                }
            || format_micros(slot_micros) <= armed_at
        {
            continue;
        }
        let slot_text = format_micros(slot_micros);
        // A failing schedule rolls back alone: its claim is retried next pass, the others still run.
        ctx.conn.execute_batch("SAVEPOINT schedule_run")?;
        let outcome = (|| -> Result<bool> {
            if ctx.conn.execute(
                "UPDATE schedules SET last_slot_at=?1 WHERE id=?2 AND (last_slot_at IS NULL OR last_slot_at < ?1)",
                rusqlite::params![slot_text, id],
            )? == 1 {
                let _ = run_recorded(ctx, id, Some(&slot_text))?;
                return Ok(true);
            }
            Ok(false)
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

const fn leap_year(year: i32) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}
const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

// Howard Hinnant's civil calendar algorithms; day zero is 1970-01-01.
const fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let year = year as i64 - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i64;
    let day = day as i64;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_part = (5 * doy + 2) / 153;
    let day = doy - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "UPDATE schedules SET action='fix' WHERE id=?1",
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
            .execute("UPDATE schedules SET action='fix'", [])
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
    fn latest_slot_matches_python_boundaries() {
        let mask = days_to_mask(&[0, 2, 4]);
        assert_eq!(
            latest_slot(local(2026, 10, 24, 12, 0), mask, time("03:00")),
            Some(local(2026, 10, 23, 3, 0))
        );
        assert_eq!(
            latest_slot(local(2026, 10, 23, 2, 59), mask, time("03:00")),
            Some(local(2026, 10, 21, 3, 0))
        );
        assert_eq!(
            latest_slot(local(2026, 10, 23, 3, 0), mask, time("03:00")),
            Some(local(2026, 10, 23, 3, 0))
        );
    }

    #[test]
    fn next_slot_is_strictly_after_now() {
        let mask = days_to_mask(&[0, 2, 4]);
        assert_eq!(
            next_slot(local(2026, 10, 24, 12, 0), mask, time("03:00")),
            Some(local(2026, 10, 26, 3, 0))
        );
        assert_eq!(
            next_slot(local(2026, 10, 26, 2, 0), mask, time("03:00")),
            Some(local(2026, 10, 26, 3, 0))
        );
        assert_eq!(
            next_slot(local(2026, 10, 26, 3, 0), mask, time("03:00")),
            Some(local(2026, 10, 28, 3, 0))
        );
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
    fn cron_runs_agree_with_weekly_runs_across_dst_boundaries() {
        let timezone = chrono_tz::Europe::Zurich;
        let mask = days_to_mask(&[0, 1, 2, 3, 4, 5, 6]);
        let cron = effective_cron(None, mask, "02:30").unwrap();
        for (year, month, day) in [(2026, 3, 28), (2026, 3, 29), (2026, 10, 24), (2026, 10, 25)] {
            for hour in 0..24 {
                let now = utc(year, month, day, hour, 0);
                assert_eq!(
                    next_cron_run_at(now, true, &cron, &timezone),
                    next_run_at(now, true, mask, time("02:30"), &timezone),
                    "{now}"
                );
            }
        }
        assert!(next_cron_run_at(utc(2026, 3, 29, 0, 0), false, &cron, &timezone).is_none());
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
    fn next_run_is_future_across_dst_boundaries() {
        let timezone = chrono_tz::Europe::Zurich;
        let mask = days_to_mask(&[0, 1, 2, 3, 4, 5, 6]);
        for (year, month, day) in [(2026, 3, 28), (2026, 3, 29), (2026, 10, 24), (2026, 10, 25)] {
            for hour in 0..24 {
                let now = utc(year, month, day, hour, 0);
                let following = next_run_at(now, true, mask, time("02:30"), &timezone).unwrap();
                assert!(
                    following > now && following <= now + chrono::TimeDelta::hours(25),
                    "{now} -> {following}"
                );
            }
        }
        assert!(
            next_run_at(
                utc(2026, 3, 29, 0, 0),
                false,
                mask,
                time("02:30"),
                &timezone
            )
            .is_none()
        );
    }
}

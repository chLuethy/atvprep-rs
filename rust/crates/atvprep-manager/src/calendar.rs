//! Pure local-calendar primitives for schedules: the wall-clock time of a slot and its conversion to
//! UTC, and the next slot of a cron expression.
//!
//! The worker deliberately keeps a scheduled wall-clock time separate from its
//! UTC conversion.  The latter depends on the host timezone and DST policy;
//! these primitives are deterministic and use Monday as weekday zero, exactly
//! like Python's `datetime.weekday()`. Both [`crate::schedules`] (the dispatch) and
//! [`crate::schedule_due`] (the decision) build on them, so neither needs the other for the calendar.

use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDate, Offset, TimeZone, Timelike, Utc};

use crate::cron::CronExpr;
use crate::{Error, Result};

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

/// The local wall-clock minute of an instant in `timezone`.
#[must_use]
pub fn local_of<Tz>(moment: DateTime<Utc>, timezone: &Tz) -> Option<LocalDateTime>
where
    Tz: TimeZone,
{
    let local = moment.with_timezone(timezone);
    LocalDateTime::new(
        local.year(),
        u8::try_from(local.month()).ok()?,
        u8::try_from(local.day()).ok()?,
        u8::try_from(local.hour()).ok()?,
        u8::try_from(local.minute()).ok()?,
    )
    .ok()
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
/// DST is handled by [`local_to_utc`]: a repeated wall-clock time is its first occurrence, a
/// skipped one uses the offset before the gap.
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
    let mut slot = cron.next_slot(local_of(now, timezone)?);
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

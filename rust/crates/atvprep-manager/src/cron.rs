//! A small standard 5-field cron: `minute hour day-of-month month day-of-week`.
//!
//! Numbers only (no names, no `@macros`, no seconds). Weekday 0 and 7 are both Sunday. As in
//! classic cron, when day-of-month and day-of-week are both restricted, a day matches if
//! either does. Slots are naive local times; the DST mapping stays with
//! [`crate::schedules::local_to_utc`].

use crate::schedules::{LocalDateTime, TimeOfDay};
use crate::{Error, Result};

mod describe;
pub use describe::Lang;

/// How many days a slot search looks over: enough for 29 February on any weekday.
const SEARCH_DAYS: i64 = 366 * 9;

/// A parsed cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpr {
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    days_any: bool,
    weekdays_any: bool,
    text: String,
}

fn invalid(reason: &str) -> Error {
    Error::Invalid(format!("invalid cron expression: {reason}"))
}

/// A field with its name and range, so messages say what is wrong where.
#[derive(Clone, Copy)]
struct Spec {
    name: &'static str,
    min: u32,
    max: u32,
}

impl Spec {
    fn error(self, problem: &str) -> Error {
        invalid(&format!("{}: {problem}", self.name))
    }
}

const MINUTE: Spec = Spec {
    name: "minute",
    min: 0,
    max: 59,
};
const HOUR: Spec = Spec {
    name: "hour",
    min: 0,
    max: 23,
};
const DAY: Spec = Spec {
    name: "day of month",
    min: 1,
    max: 31,
};
const MONTH: Spec = Spec {
    name: "month",
    min: 1,
    max: 12,
};
const WEEKDAY: Spec = Spec {
    name: "weekday",
    min: 0,
    max: 7,
};

fn number(text: &str, spec: Spec, min: u32) -> Result<u32> {
    if text.is_empty() || text.len() > 2 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(spec.error(&format!(
            "\"{text}\" is not a number (names and macros are not supported)"
        )));
    }
    let value: u32 = text
        .parse()
        .map_err(|_| spec.error(&format!("\"{text}\" is not a number")))?;
    if (min..=spec.max).contains(&value) {
        Ok(value)
    } else {
        Err(spec.error(&format!(
            "{value} is out of range {}-{}",
            spec.min, spec.max
        )))
    }
}

/// Returns the bit set of a field and whether it starts with `*`.
fn field(text: &str, spec: Spec) -> Result<(u64, bool)> {
    let (min, max) = (spec.min, spec.max);
    let mut bits = 0u64;
    for item in text.split(',') {
        let (base, step) = match item.split_once('/') {
            Some((base, step)) => (base, Some(number(step, spec, 1)?)),
            None => (item, None),
        };
        let (from, to) = if base == "*" {
            (min, max)
        } else if let Some((from, to)) = base.split_once('-') {
            let (from, to) = (number(from, spec, min)?, number(to, spec, min)?);
            if from > to {
                return Err(spec.error(&format!("descending range {from}-{to}")));
            }
            (from, to)
        } else {
            let from = number(base, spec, min)?;
            (from, if step.is_some() { max } else { from })
        };
        let step = step.unwrap_or(1);
        let mut value = from;
        while value <= to {
            bits |= 1 << value;
            value += step;
        }
    }
    Ok((bits, text.starts_with('*')))
}

impl CronExpr {
    /// Parse a 5-field cron expression.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for anything but five numeric fields in range.
    pub fn parse(text: &str) -> Result<Self> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = parts[..] else {
            return Err(invalid(&format!(
                "expected 5 fields (minute hour day-of-month month weekday), found {}",
                parts.len()
            )));
        };
        let (minutes, _) = field(minute, MINUTE)?;
        let (hours, _) = field(hour, HOUR)?;
        let (days, days_any) = field(day, DAY)?;
        let (months, _) = field(month, MONTH)?;
        let (weekdays, weekdays_any) = field(weekday, WEEKDAY)?;
        // 7 is Sunday too.
        let weekdays = (weekdays & 0x7f) | ((weekdays >> 7) & 1);
        Ok(Self {
            minutes,
            hours: u32::try_from(hours).unwrap_or(0),
            days: u32::try_from(days).unwrap_or(0),
            months: u16::try_from(months).unwrap_or(0),
            weekdays: u8::try_from(weekdays).unwrap_or(0),
            days_any,
            weekdays_any,
            text: parts.join(" "),
        })
    }

    /// The cron of a weekly schedule: weekday mask (Monday = 0) and time of day.
    ///
    /// # Panics
    ///
    /// Never in practice: the generated text is always a valid expression.
    #[must_use]
    pub fn from_weekdays(days_mask: u8, time: TimeOfDay) -> Self {
        // Cron counts Sunday as 0, the mask counts Monday as 0.
        let mut list: Vec<u8> = (0..7)
            .filter(|day| days_mask & (1 << day) != 0)
            .map(|day| (day + 1) % 7)
            .collect();
        list.sort_unstable();
        let list: Vec<String> = list.iter().map(u8::to_string).collect();
        let text = format!("{} {} * * {}", time.minute(), time.hour(), list.join(","));
        Self::parse(&text).expect("a generated cron expression is valid")
    }

    /// The normalised expression text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// A weekday mask (Monday = 0) and time that stand in for this cron in the legacy columns.
    ///
    /// # Panics
    ///
    /// Never in practice: the first hour and minute are always in range.
    #[must_use]
    pub fn legacy_columns(&self) -> (u8, TimeOfDay) {
        let first = |bits: u64| u8::try_from(bits.trailing_zeros()).unwrap_or(0);
        let time = TimeOfDay::parse(&format!(
            "{:02}:{:02}",
            first(u64::from(self.hours)),
            first(self.minutes)
        ))
        .expect("first hour and minute are in range");
        let mask = if self.weekdays_any || !self.days_any {
            127
        } else {
            (0..7u8)
                .filter(|day| self.weekdays & (1 << ((day + 1) % 7)) != 0)
                .fold(0, |mask, day| mask | (1 << day))
        };
        (mask, time)
    }

    /// The smallest gap in minutes between two consecutive slots, or `None` if it never runs.
    ///
    /// Within a day the gaps depend only on the hour and minute sets. Across midnight a gap only
    /// counts on two consecutive matching days, found by walking the same span as the slot search.
    #[must_use]
    pub fn min_interval_minutes(&self) -> Option<u32> {
        let mut starts: Vec<u32> = Vec::new();
        for hour in (0..24u32).filter(|h| self.hours & (1 << h) != 0) {
            for minute in (0..60u32).filter(|m| self.minutes & (1 << m) != 0) {
                starts.push(hour * 60 + minute);
            }
        }
        let (first, last) = (*starts.first()?, *starts.last()?);
        let first_day = LocalDateTime::new(2024, 1, 1, 0, 0).ok()?;
        let mut matching =
            (0..SEARCH_DAYS).map(|ahead| self.day_matches(first_day.add_days(ahead)));
        let mut previous = matching.next()?;
        let mut any = previous;
        let mut closest = u32::MAX;
        for today in matching {
            if previous && today {
                closest = closest.min(24 * 60 - last + first);
            }
            any |= today;
            previous = today;
        }
        if !any {
            return None;
        }
        let within = starts.windows(2).map(|pair| pair[1] - pair[0]).min();
        // A repeating gap of a week or a day, whichever the days allow, is never the closest.
        Some(closest.min(within.unwrap_or(u32::MAX)).min(7 * 24 * 60))
    }

    fn day_matches(&self, day: LocalDateTime) -> bool {
        if self.months & (1 << day.month()) == 0 {
            return false;
        }
        let in_month = self.days & (1 << day.day()) != 0;
        let in_week = self.weekdays & (1 << ((day.weekday() + 1) % 7)) != 0;
        match (self.days_any, self.weekdays_any) {
            (true, true) => true,
            (true, false) => in_week,
            (false, true) => in_month,
            (false, false) => in_month || in_week,
        }
    }

    fn slot_on(day: LocalDateTime, hour: u8, minute: u8) -> Option<LocalDateTime> {
        LocalDateTime::new(day.year(), day.month(), day.day(), hour, minute).ok()
    }

    /// The latest slot not later than `now`.
    #[must_use]
    pub fn latest_slot(&self, now: LocalDateTime) -> Option<LocalDateTime> {
        for back in 0..SEARCH_DAYS {
            let day = now.add_days(-back);
            if !self.day_matches(day) {
                continue;
            }
            for hour in (0..24u8).rev().filter(|h| self.hours & (1 << h) != 0) {
                for minute in (0..60u8).rev().filter(|m| self.minutes & (1 << m) != 0) {
                    if let Some(slot) = Self::slot_on(day, hour, minute)
                        && slot <= now
                    {
                        return Some(slot);
                    }
                }
            }
        }
        None
    }

    /// The first slot strictly later than `now`.
    #[must_use]
    pub fn next_slot(&self, now: LocalDateTime) -> Option<LocalDateTime> {
        for ahead in 0..SEARCH_DAYS {
            let day = now.add_days(ahead);
            if !self.day_matches(day) {
                continue;
            }
            for hour in (0..24u8).filter(|h| self.hours & (1 << h) != 0) {
                for minute in (0..60u8).filter(|m| self.minutes & (1 << m) != 0) {
                    if let Some(slot) = Self::slot_on(day, hour, minute)
                        && slot > now
                    {
                        return Some(slot);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u8, d: u8, h: u8, mi: u8) -> LocalDateTime {
        LocalDateTime::new(y, mo, d, h, mi).unwrap()
    }
    fn cron(text: &str) -> CronExpr {
        CronExpr::parse(text).unwrap()
    }

    // 2026-10-25 is a Sunday, 2026-10-26 a Monday.
    #[test]
    fn min_interval_finds_the_closest_two_slots() {
        for (text, minutes) in [
            ("* * * * *", Some(1)),
            ("0,5 * * * *", Some(5)),
            ("0,10 * * * *", Some(10)),
            ("*/15 * * * *", Some(15)),
            ("0 */6 * * *", Some(360)),
            ("0 3 * * *", Some(1440)),
            ("0 3 * * 1", Some(10080)),
            // Across midnight: 23:55 to 00:05 the next day.
            ("55,5 23,0 * * *", Some(10)),
            // Only on a leap day does the 5-minute pair exist.
            ("0,5 0 29 2 *", Some(5)),
            // 31 February never happens.
            ("0 0 31 2 *", None),
        ] {
            assert_eq!(cron(text).min_interval_minutes(), minutes, "{text}");
        }
    }

    #[test]
    fn sunday_is_zero_and_seven() {
        for text in ["0 3 * * 0", "0 3 * * 7"] {
            assert_eq!(
                cron(text).next_slot(at(2026, 10, 24, 12, 0)),
                Some(at(2026, 10, 25, 3, 0)),
                "{text}"
            );
        }
    }

    #[test]
    fn ranges_lists_and_steps() {
        let weekdays = cron("30 2 * * 1-5");
        assert_eq!(
            weekdays.next_slot(at(2026, 10, 23, 3, 0)),
            Some(at(2026, 10, 26, 2, 30))
        );
        let list = cron("0 6,18 * * *");
        assert_eq!(
            list.next_slot(at(2026, 10, 24, 7, 0)),
            Some(at(2026, 10, 24, 18, 0))
        );
        let step = cron("*/15 * * * *");
        assert_eq!(
            step.latest_slot(at(2026, 10, 24, 7, 44)),
            Some(at(2026, 10, 24, 7, 30))
        );
        assert_eq!(
            cron("10/20 0 * * *").next_slot(at(2026, 10, 24, 0, 11)),
            Some(at(2026, 10, 24, 0, 30))
        );
        assert_eq!(
            cron("0 0 * * 5-7").latest_slot(at(2026, 10, 25, 0, 0)),
            Some(at(2026, 10, 25, 0, 0))
        );
    }

    #[test]
    fn day_of_month_and_weekday_match_either_when_both_are_set() {
        let either = cron("0 0 1 * 1");
        assert_eq!(
            either.next_slot(at(2026, 10, 24, 0, 0)),
            Some(at(2026, 10, 26, 0, 0))
        );
        assert_eq!(
            either.next_slot(at(2026, 10, 26, 0, 0)),
            Some(at(2026, 11, 1, 0, 0))
        );
    }

    #[test]
    fn rare_dates_are_found() {
        assert_eq!(
            cron("0 0 29 2 *").next_slot(at(2026, 3, 1, 0, 0)),
            Some(at(2028, 2, 29, 0, 0))
        );
    }

    #[test]
    fn invalid_expressions_are_rejected() {
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * 32 * *",
            "* * * 13 *",
            "* * * * 8",
            "5-1 * * * *",
            "*/0 * * * *",
            "a * * * *",
            "@daily",
            "* * * jan *",
            "* * * * mon",
            "1,,2 * * * *",
            "-1 * * * *",
            "1- * * * *",
            "*/ * * * *",
        ] {
            assert!(CronExpr::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn weekday_mask_becomes_a_cron_with_sunday_zero() {
        let time = TimeOfDay::parse("03:05").unwrap();
        // Monday and Sunday.
        assert_eq!(
            CronExpr::from_weekdays(0b100_0001, time).as_str(),
            "5 3 * * 0,1"
        );
        assert_eq!(
            CronExpr::from_weekdays(127, time).as_str(),
            "5 3 * * 0,1,2,3,4,5,6"
        );
    }

    #[test]
    fn legacy_columns_round_trip_a_weekly_cron() {
        let time = TimeOfDay::parse("12:30").unwrap();
        for mask in 1..=127u8 {
            assert_eq!(
                CronExpr::from_weekdays(mask, time).legacy_columns(),
                (mask, time)
            );
        }
    }

    #[test]
    fn errors_name_the_field_and_the_problem() {
        let message = |text: &str| CronExpr::parse(text).unwrap_err().to_string();
        assert!(
            message("* * * *").contains("5 fields"),
            "{}",
            message("* * * *")
        );
        assert!(
            message("61 * * * *").contains("minute"),
            "{}",
            message("61 * * * *")
        );
        assert!(message("61 * * * *").contains("0-59"));
        assert!(message("* 24 * * *").contains("hour"));
        assert!(message("* * 32 * *").contains("day of month"));
        assert!(message("* * * 13 *").contains("month"));
        assert!(message("* * * * 8").contains("weekday"));
        assert!(message("* * * * mon").contains("weekday"));
        assert!(message("5-1 * * * *").contains("descending"));
    }

    fn say(text: &str, lang: Lang) -> String {
        cron(text).describe(lang)
    }

    #[test]
    fn describes_common_expressions_in_english() {
        assert_eq!(say("0 3 * * *", Lang::En), "Every day at 03:00");
        assert_eq!(say("0 3 * * 1-5", Lang::En), "At 03:00, on weekdays");
        assert_eq!(say("*/15 * * * *", Lang::En), "Every 15 minutes");
        assert_eq!(say("* * * * *", Lang::En), "Every minute");
        assert_eq!(
            say("0 */6 * * *", Lang::En),
            "At 00:00, 06:00, 12:00, 18:00, every day"
        );
        assert_eq!(say("30 * * * *", Lang::En), "Every hour at minute 30");
        assert_eq!(say("0 * * * *", Lang::En), "Every hour");
        assert_eq!(say("0 4 * * 0,7", Lang::En), "At 04:00, on Sunday");
        assert_eq!(say("0 4 * * 6,0", Lang::En), "At 04:00, on weekends");
        assert_eq!(
            say("15 2 1,15 * *", Lang::En),
            "At 02:15, on day 1, 15 of the month"
        );
        assert_eq!(
            say("0 9 * 1,7 1", Lang::En),
            "At 09:00, on Monday, in January, July"
        );
        assert_eq!(
            say("0 0 1 * 1", Lang::En),
            "At 00:00, on day 1 of the month or on Monday"
        );
    }

    #[test]
    fn describes_common_expressions_in_german() {
        assert_eq!(say("0 3 * * *", Lang::De), "Täglich um 03:00");
        assert_eq!(say("0 3 * * 1-5", Lang::De), "Um 03:00, an Werktagen");
        assert_eq!(say("*/15 * * * *", Lang::De), "Alle 15 Minuten");
        assert_eq!(say("* * * * *", Lang::De), "Jede Minute");
        assert_eq!(say("0 4 * * 3", Lang::De), "Um 04:00, am Mittwoch");
    }

    #[test]
    fn many_times_fall_back_to_minutes_and_hours() {
        assert_eq!(
            say("0,10,20 9-17 * * *", Lang::En),
            "At minutes 0, 10, 20 of hours 9-17, every day"
        );
    }
}

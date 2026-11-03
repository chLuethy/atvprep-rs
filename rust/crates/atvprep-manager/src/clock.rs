//! Time as the database stores it: UTC text `YYYY-MM-DD HH:MM:SS.ffffff`, which sorts like time.

use std::time::{SystemTime, UNIX_EPOCH};

/// Where "now" comes from; tests pin it.
pub trait Clock: Send + Sync {
    /// Microseconds since the Unix epoch.
    fn now_micros(&self) -> i64;

    fn now(&self) -> String {
        format_micros(self.now_micros())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_micros(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn now_micros(&self) -> i64 {
        self.0
    }
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian calendar.
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `YYYY-MM-DD HH:MM:SS.ffffff` for microseconds since the epoch.
#[must_use]
pub fn format_micros(micros: i64) -> String {
    let secs = micros.div_euclid(1_000_000);
    let frac = micros.rem_euclid(1_000_000);
    let (year, month, day) = civil(secs.div_euclid(86_400));
    let rest = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{frac:06}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_sqlite_text_and_sorts_like_time() {
        assert_eq!(format_micros(0), "1970-01-01 00:00:00.000000");
        assert_eq!(
            format_micros(1_700_000_000_123_456),
            "2023-11-14 22:13:20.123456"
        );
        assert_eq!(
            format_micros(951_782_400_000_000),
            "2000-02-29 00:00:00.000000"
        );
        assert!(format_micros(999_999) < format_micros(1_000_000));
        assert_eq!(FixedClock(5_000_000).now(), "1970-01-01 00:00:05.000000");
    }
}

//! Plain-language descriptions of a cron expression, in English or German.

use super::CronExpr;

/// The language of [`CronExpr::describe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
}

impl Lang {
    /// `"de"` is German; anything else is English.
    #[must_use]
    pub fn from_code(code: &str) -> Self {
        if code.eq_ignore_ascii_case("de") {
            Self::De
        } else {
            Self::En
        }
    }

    fn pick(self, en: &'static str, de: &'static str) -> &'static str {
        match self {
            Self::En => en,
            Self::De => de,
        }
    }
}

const WEEKDAYS_EN: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const WEEKDAYS_DE: [&str; 7] = [
    "Sonntag",
    "Montag",
    "Dienstag",
    "Mittwoch",
    "Donnerstag",
    "Freitag",
    "Samstag",
];
const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_DE: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];

fn members(bits: u64, min: usize, max: usize) -> Vec<usize> {
    (min..=max).filter(|v| bits & (1 << v) != 0).collect()
}

/// `1, 2, 3, 5` as `1-3, 5`: runs of three or more collapse into a range.
fn compress(values: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < values.len() {
        let mut j = i;
        while j + 1 < values.len() && values[j + 1] == values[j] + 1 {
            j += 1;
        }
        if j >= i + 2 {
            parts.push(format!("{}-{}", values[i], values[j]));
        } else {
            parts.extend(values[i..=j].iter().map(usize::to_string));
        }
        i = j + 1;
    }
    parts.join(", ")
}

/// The constant step of an evenly spaced list that starts at 0 and divides 60, if any.
fn even_step(values: &[usize]) -> Option<usize> {
    let step = *values.get(1)?;
    let evenly = values.iter().enumerate().all(|(i, v)| i * step == *v);
    (values[0] == 0 && evenly && 60 % step == 0).then_some(step)
}

impl CronExpr {
    /// A plain-language description in `lang`, for people who do not read cron.
    #[must_use]
    pub fn describe(&self, lang: Lang) -> String {
        let minutes = members(self.minutes, 0, 59);
        let hours = members(u64::from(self.hours), 0, 23);
        let days = members(u64::from(self.days), 1, 31);
        let months = members(u64::from(self.months), 1, 12);
        let weekdays = members(u64::from(self.weekdays), 0, 6);
        let any_day = self.days_any && self.weekdays_any;
        let all_days = any_day && months.len() == 12;

        let times: Vec<String> = hours
            .iter()
            .flat_map(|h| minutes.iter().map(move |m| format!("{h:02}:{m:02}")))
            .collect();
        // `whole` marks a phrase that already says how often, so no "every day" follows it.
        let (time, whole) = if hours.len() == 24 && minutes.len() == 60 {
            (lang.pick("Every minute", "Jede Minute").to_owned(), true)
        } else if hours.len() == 24 && minutes.len() > 1 {
            let text = match (even_step(&minutes), lang) {
                (Some(step), Lang::En) => format!("Every {step} minutes"),
                (Some(step), Lang::De) => format!("Alle {step} Minuten"),
                (None, Lang::En) => format!("Every hour at minutes {}", compress(&minutes)),
                (None, Lang::De) => {
                    format!("Jede Stunde zu den Minuten {}", compress(&minutes))
                }
            };
            (text, true)
        } else if hours.len() == 24 {
            let text = match (minutes[0], lang) {
                (0, _) => lang.pick("Every hour", "Jede Stunde").to_owned(),
                (m, Lang::En) => format!("Every hour at minute {m}"),
                (m, Lang::De) => format!("Jede Stunde zur Minute {m}"),
            };
            (text, true)
        } else if times.len() <= 8 {
            (
                format!("{} {}", lang.pick("At", "Um"), times.join(", ")),
                false,
            )
        } else {
            let (m, h) = (compress(&minutes), compress(&hours));
            let text = match lang {
                Lang::En => format!("At minutes {m} of hours {h}"),
                Lang::De => format!("Zu den Minuten {m} der Stunden {h}"),
            };
            (text, false)
        };

        if all_days && times.len() == 1 && !whole {
            return match lang {
                Lang::En => format!("Every day at {}", times[0]),
                Lang::De => format!("Täglich um {}", times[0]),
            };
        }
        if whole && all_days {
            return time;
        }
        let mut parts = vec![time];
        if any_day {
            if !whole {
                parts.push(lang.pick("every day", "jeden Tag").to_owned());
            }
        } else {
            parts.push(Self::describe_days(
                lang,
                &days,
                &weekdays,
                (self.days_any, self.weekdays_any),
            ));
        }
        if months.len() != 12 {
            let names = if lang == Lang::En {
                MONTHS_EN
            } else {
                MONTHS_DE
            };
            let list: Vec<&str> = months.iter().map(|m| names[m - 1]).collect();
            parts.push(format!("{} {}", lang.pick("in", "im"), list.join(", ")));
        }
        parts.join(", ")
    }

    fn describe_days(
        lang: Lang,
        days: &[usize],
        weekdays: &[usize],
        (days_any, weekdays_any): (bool, bool),
    ) -> String {
        let names = if lang == Lang::En {
            WEEKDAYS_EN
        } else {
            WEEKDAYS_DE
        };
        let week = match weekdays {
            [1, 2, 3, 4, 5] => lang.pick("on weekdays", "an Werktagen").to_owned(),
            [0, 6] => lang.pick("on weekends", "am Wochenende").to_owned(),
            [0, 1, 2, 3, 4, 5, 6] => lang.pick("every day", "jeden Tag").to_owned(),
            list => {
                let list: Vec<&str> = list.iter().map(|d| names[*d]).collect();
                format!("{} {}", lang.pick("on", "am"), list.join(", "))
            }
        };
        let month = match lang {
            Lang::En => format!("on day {} of the month", compress(days)),
            Lang::De => format!("am {}. des Monats", compress(days).replace(", ", "., ")),
        };
        match (days_any, weekdays_any) {
            (true, _) => week,
            (false, true) => month,
            (false, false) => format!("{month} {} {week}", lang.pick("or", "oder")),
        }
    }
}

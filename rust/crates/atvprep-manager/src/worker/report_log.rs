//! Step lines for an item's log, made from what a tool was given and what it reported.
use super::process::{PrepareMode, ProcessResult, StopReason};
use crate::{Error, state_machine::ItemStatus};
use serde_json::Value;
use std::{path::Path, time::Duration};

/// `yes` or `no`, as a log line spells a setting.
pub(crate) const fn yes_no(set: bool) -> &'static str {
    if set { "yes" } else { "no" }
}

/// The opening lines of an item: what runs and on which file.
pub(super) fn opening(action: &str, relative: &str) -> [String; 2] {
    [format!("Start: {action}"), format!("File: {relative}")]
}

/// The frame of a Vorbereiten run: Kodierfreigabe, Zielprofil, Original löschen and a forced rebuild.
pub(super) fn prepare_frame(mode: PrepareMode) -> Vec<String> {
    let mut lines = vec![
        format!("Encode allowance: {}", mode.encode.as_str()),
        format!("Target profile: {}", mode.profile.summary()),
        format!("Delete original: {}", yes_no(mode.delete_original)),
    ];
    if mode.force {
        lines.push("Force: yes".to_owned());
    }
    lines
}

/// What a check stored for the file: state, Kodierbedarf and Änderungsart, `unknown` where unset.
pub(super) fn stored_verdict(state: &str, need: Option<&str>, change: Option<&str>) -> [String; 3] {
    [
        format!("State: {state}"),
        format!("Encode need: {}", need.unwrap_or("unknown")),
        format!("Planned change: {}", change.unwrap_or("unknown")),
    ]
}

/// How the tool ended and how long the item took.
pub(super) fn ending(exit_code: Option<i32>, elapsed: Duration) -> [String; 2] {
    [
        format!(
            "Exit code: {}",
            exit_code.map_or_else(|| "none".to_owned(), |code| code.to_string())
        ),
        format!("Duration: {}", duration(elapsed)),
    ]
}

/// The last line of an item that ended as `terminal`: why it stopped, failed (`code` and the
/// `error` the CLI reported, or what the process tells), or the `refusal` of a file above its
/// Kodierfreigabe. `None` when nothing more is to be said.
pub(super) fn closing(
    terminal: ItemStatus,
    code: Option<&str>,
    process: &ProcessResult,
    error: Option<&str>,
    refusal: Option<&str>,
) -> Option<String> {
    match terminal {
        ItemStatus::Interrupted => Some("Interrupted: the manager is shutting down".to_owned()),
        ItemStatus::Cancelled => Some("Cancelled".to_owned()),
        ItemStatus::Failed => Some(format!(
            "Failed: {}: {}",
            code.unwrap_or("failed"),
            match (process.reason, process.exit_code) {
                (StopReason::TimedOut, _) => "the tool ran out of time".to_owned(),
                (_, Some(_)) if let Some(error) = error => error.to_owned(),
                (_, Some(exit)) => format!("the tool exited with code {exit}"),
                (_, None) => "the tool stopped without an exit code".to_owned(),
            }
        )),
        _ => refusal.map(str::to_owned),
    }
}

/// The last line of an item an error ended.
pub(super) fn failed_by(error: &Error) -> String {
    format!("Failed: {}: {error}", error_code(error))
}

/// `Command: <tool> <arguments>`, the tool named by its file name alone.
pub(super) fn command(executable: &Path, arguments: &[String]) -> String {
    let tool = executable
        .file_name()
        .map_or_else(|| "tool".into(), |name| name.to_string_lossy());
    let mut line = format!("Command: {tool}");
    for argument in arguments {
        line.push(' ');
        if argument.is_empty() || argument.chars().any(char::is_whitespace) {
            line.push('\'');
            line.push_str(&argument.replace('\'', r"'\''"));
            line.push('\'');
        } else {
            line.push_str(argument);
        }
    }
    line
}

/// `4.21 s` below a minute, `m:ss` below an hour, `h:mm:ss` beyond.
pub(super) fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{:.2} s", elapsed.as_secs_f64())
    } else if seconds < 3600 {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds % 3600 / 60,
            seconds % 60
        )
    }
}

/// The code of an error that ended an item, for its last log line.
pub(super) const fn error_code(error: &Error) -> &'static str {
    match error {
        Error::Busy => "busy",
        Error::Constraint(_) => "constraint",
        Error::NotFound(_) => "not_found",
        Error::Invalid(_) => "invalid",
        Error::Illegal(_) => "illegal",
        Error::Conflict(_) => "conflict",
        Error::Sqlite(_) => "database",
    }
}

/// One line per stream of an ffprobe report: type, codec, channels, sample rate, language and the
/// dispositions that are set.
pub(super) fn streams(report: &Value) -> Vec<String> {
    let Some(streams) = report.get("streams").and_then(Value::as_array) else {
        return vec!["Streams: none reported".to_owned()];
    };
    if streams.is_empty() {
        return vec!["Streams: none".to_owned()];
    }
    streams
        .iter()
        .enumerate()
        .map(|(position, stream)| {
            let text = |key: &str| stream.get(key).and_then(Value::as_str).unwrap_or("unknown");
            let index = stream
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or(i64::try_from(position).unwrap_or_default());
            let mut parts = vec![format!(
                "Stream {index}: {} {}",
                text("codec_type"),
                text("codec_name")
            )];
            if let Some(channels) = stream.get("channels").and_then(Value::as_i64) {
                parts.push(format!("{channels} channels"));
            }
            if let Some(rate) = stream.get("sample_rate").and_then(Value::as_str) {
                parts.push(format!("{rate} Hz"));
            }
            if let Some(language) = stream
                .get("tags")
                .and_then(|tags| tags.get("language"))
                .and_then(Value::as_str)
            {
                parts.push(format!("language {language}"));
            }
            if let Some(disposition) = stream.get("disposition").and_then(Value::as_object) {
                parts.extend(
                    disposition
                        .iter()
                        .filter(|(_, set)| set.as_i64() == Some(1))
                        .map(|(flag, _)| flag.clone()),
                );
            }
            parts.join(", ")
        })
        .collect()
}

/// One line per compliance rule of a verify report: `ok`, or each of its findings. Findings of a
/// rule this build does not know follow at the end.
pub(super) fn rules(report: &Value) -> Vec<String> {
    let violations: Vec<&Value> = report
        .get("violations")
        .and_then(Value::as_array)
        .map(|all| all.iter().collect())
        .unwrap_or_default();
    let rule_of = |violation: &Value| {
        violation
            .get("rule")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned()
    };
    let finding = |violation: &Value| {
        let text = |key: &str| violation.get(key).and_then(Value::as_str).unwrap_or("");
        let track = violation
            .get("track_id")
            .and_then(Value::as_i64)
            .map_or_else(String::new, |track| format!(" (track {track})"));
        format!(
            "Rule {}: {}{track}: {}",
            rule_of(violation),
            text("severity"),
            text("message")
        )
    };
    let names: Vec<&'static str> = atvprep_core::compliance::rules::all_rules()
        .iter()
        .map(|rule| rule.name())
        .collect();
    let mut lines = Vec::new();
    for name in &names {
        let found: Vec<String> = violations
            .iter()
            .filter(|violation| rule_of(violation) == *name)
            .map(|violation| finding(violation))
            .collect();
        if found.is_empty() {
            lines.push(format!("Rule {name}: ok"));
        } else {
            lines.extend(found);
        }
    }
    lines.extend(
        violations
            .iter()
            .filter(|violation| !names.contains(&rule_of(violation).as_str()))
            .map(|violation| finding(violation)),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_a_clock_beyond_a_minute() {
        assert_eq!(duration(Duration::from_millis(4210)), "4.21 s");
        assert_eq!(duration(Duration::from_secs(125)), "2:05");
        assert_eq!(duration(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn arguments_with_spaces_are_quoted() {
        assert_eq!(
            command(
                Path::new("/opt/bin/atvprep"),
                &["verify".to_owned(), "My Film.mp4".to_owned()]
            ),
            "Command: atvprep verify 'My Film.mp4'"
        );
    }
}

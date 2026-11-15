//! Step lines of a Vorbereiten run, read from what the media CLI reports while it runs.
//!
//! The CLI prints machine lines on stdout (see `atvprep_core::stages`); with
//! `ATVPREP_TRACE_PHASES=1` the phase timing writes one JSON line per phase on stderr. Both pipes
//! are read line by line on their own, so output of one never breaks a line of the other.
use super::{process::Stream, report_log};
use crate::clock::Clock;
use atvprep_core::stages::{ERROR_MARKER, PLAN_MARKER, VERIFY_MARKER};
use std::time::Duration;

/// Reads the CLI's reports out of its output, chunk by chunk.
#[derive(Default)]
pub(super) struct PrepareReports {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    error: Option<String>,
}

impl PrepareReports {
    /// The step lines the complete lines of `chunk` add, now on `clock`, keeping the error the
    /// CLI reports among them for [`Self::error`].
    pub fn step_lines_keeping_error(
        &mut self,
        stream: Stream,
        chunk: &[u8],
        clock: &dyn Clock,
    ) -> Vec<String> {
        let buffer = match stream {
            Stream::Stdout => &mut self.stdout,
            Stream::Stderr => &mut self.stderr,
        };
        buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            lines.push(String::from_utf8_lossy(&line).trim().to_owned());
        }
        // A line longer than any report is tool output; it is not kept around.
        if buffer.len() > 64 * 1024 {
            buffer.clear();
        }
        lines
            .iter()
            .filter_map(|line| match stream {
                Stream::Stdout => self.machine(line),
                Stream::Stderr => phase(line, clock),
            })
            .collect()
    }

    /// The error the CLI reported for the file, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn machine(&mut self, line: &str) -> Option<String> {
        if let Some(plan) = line.strip_prefix(PLAN_MARKER) {
            return Some(format!("Plan {plan}"));
        }
        if line == "ATVPREP-SOURCE removed" {
            return Some("Original: removed".to_owned());
        }
        if let Some(kept) = line.strip_prefix("ATVPREP-SOURCE kept") {
            let why = kept.trim();
            return Some(if why.is_empty() {
                "Original: kept".to_owned()
            } else {
                format!("Original: kept ({why})")
            });
        }
        if let Some(checked) = line.strip_prefix(VERIFY_MARKER) {
            return Some(format!("Check after prepare: {checked}"));
        }
        if let Some(error) = line.strip_prefix(ERROR_MARKER) {
            self.error = Some(error.to_owned());
        }
        None
    }
}

/// `Phase <name>: started HH:MM:SS, took <duration>` from a phase timing record that ends now on
/// `clock`, the start in server time.
fn phase(line: &str, clock: &dyn Clock) -> Option<String> {
    let record: serde_json::Value = serde_json::from_str(line).ok()?;
    let name = record.get("phase")?.as_str()?;
    let seconds = record.get("seconds")?.as_f64()?;
    let took = Duration::try_from_secs_f64(seconds).ok()?;
    #[allow(clippy::cast_possible_truncation)] // A phase lasts far less than i64 microseconds.
    let started = clock.now_micros() - took.as_micros() as i64;
    Some(format!(
        "Phase {name}: started {}, took {}",
        clock.server_time_of_day(started),
        report_log::duration(took)
    ))
}

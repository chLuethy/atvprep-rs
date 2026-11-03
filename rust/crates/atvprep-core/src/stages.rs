//! Which stage a prepare is in, and how far along the whole file is (port of `atvprep/stages.py`).
//!
//! atvprep runs up to three stages per file: `encode` (ffmpeg), `finalize` (the moov patch and the
//! move of the moov to the front) and `verify` (compliance audit). Each reports a fraction of its
//! own, and the reporter maps it onto one percent that only grows and stays below 100 until the
//! process ends.
//!
//! With no terminal watching (the manager reads the output) it prints plain lines:
//!
//! ```text
//! ATVPREP-STAGE <name>
//! ATVPREP-PROGRESS <percent>
//! ATVPREP-OUTCOME <outcome>             (once per file, when it ends)
//! ATVPREP-SOURCE removed | kept <why>   (when the original was meant to go)
//! ```
//!
//! On a terminal it prints no protocol lines; it draws the stage and its own percent as one
//! carriage-return line on the same sink (the CLI hands it stderr).

use std::io::Write;

/// Share of the whole run, as `(start, end)` percent. Without an encode the file is only patched.
const WITH_ENCODE: [(Stage, u32, u32); 3] = [
    (Stage::Encode, 0, 90),
    (Stage::Finalize, 90, 97),
    (Stage::Verify, 97, 99),
];
const WITHOUT_ENCODE: [(Stage, u32, u32); 2] = [(Stage::Finalize, 0, 70), (Stage::Verify, 70, 99)];

/// Only the process ending proves the work is done.
const CEILING: u32 = 99;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Encode,
    Finalize,
    Verify,
}

impl Stage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Encode => "encode",
            Self::Finalize => "finalize",
            Self::Verify => "verify",
        }
    }

    /// The word shown on a terminal.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Encode => "Encoding",
            Self::Finalize => "Finalizing",
            Self::Verify => "Verifying",
        }
    }
}

/// How a file ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Patched,
    Rebuilt,
    Reencoded,
    ReencodeNeeded,
    Planned,
    Failed,
}

impl Outcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Patched => "patched",
            Self::Rebuilt => "rebuilt",
            Self::Reencoded => "reencoded",
            Self::ReencodeNeeded => "reencode_needed",
            Self::Planned => "planned",
            Self::Failed => "failed",
        }
    }
}

pub struct StageReporter {
    sink: Box<dyn Write + Send>,
    terminal: bool,
    with_encode: bool,
    stage: Option<Stage>,
    percent: u32,
    drawn: bool,
}

impl StageReporter {
    /// A reporter writing protocol lines to `sink` unless `terminal` is set.
    #[must_use]
    pub fn new(sink: Box<dyn Write + Send>, terminal: bool) -> Self {
        Self {
            sink,
            terminal,
            with_encode: true,
            stage: None,
            percent: 0,
            drawn: false,
        }
    }

    fn announce(&self) -> bool {
        !self.terminal
    }

    fn line(&mut self, text: &str) {
        if self.announce() {
            // A closed pipe must not abort the work; the reader is gone, nothing is lost.
            let _ = writeln!(self.sink, "{text}");
            let _ = self.sink.flush();
        }
    }

    /// Start a new file: percent starts over and the stage weights follow whether ffmpeg runs.
    pub fn begin_file(&mut self, encode: bool) {
        self.finish();
        self.with_encode = encode;
        self.stage = None;
        self.percent = 0;
    }

    /// Enter a stage; its fraction is 0 until [`update`](Self::update) says otherwise.
    pub fn stage(&mut self, stage: Stage) {
        if self.stage == Some(stage) {
            return;
        }
        self.stage = Some(stage);
        self.draw(&format!("{} …", stage.label()));
        self.line(&format!("ATVPREP-STAGE {}", stage.as_str()));
        self.update(0.0);
    }

    /// Report how far the current stage is (0..1); anything outside is clamped, `NaN` counts as 0.
    pub fn update(&mut self, fraction: f64) {
        let Some(stage) = self.stage else {
            return;
        };
        let weights: &[(Stage, u32, u32)] = if self.with_encode {
            &WITH_ENCODE
        } else {
            &WITHOUT_ENCODE
        };
        let Some(&(_, low, high)) = weights.iter().find(|(s, _, _)| *s == stage) else {
            return;
        };
        let fraction = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        };
        let percent = scaled(low, high, fraction).min(CEILING);
        if percent <= self.percent {
            return;
        }
        self.percent = percent;
        self.draw(&format!("{} {} %", stage.label(), shown_percent(fraction)));
        self.line(&format!("ATVPREP-PROGRESS {percent}"));
    }

    /// Say how a file ended.
    pub fn outcome(&mut self, outcome: Outcome) {
        self.line(&format!("ATVPREP-OUTCOME {}", outcome.as_str()));
    }

    /// Say whether the original was removed, or why it was kept.
    pub fn source(&mut self, removed: bool, reason: Option<&str>) {
        match (removed, reason) {
            (true, _) => self.line("ATVPREP-SOURCE removed"),
            (false, Some(why)) => self.line(&format!("ATVPREP-SOURCE kept {why}")),
            (false, None) => self.line("ATVPREP-SOURCE kept"),
        }
    }

    /// End the current file's display. Nothing is shown on a terminal yet, so this only keeps the
    /// call order of the Python original for the CLI to hook into.
    pub fn finish(&mut self) {
        if self.drawn {
            let _ = write!(self.sink, "\r\x1b[K");
            self.drawn = false;
        }
        let _ = self.sink.flush();
    }

    /// Redraw the status line; only on a terminal.
    fn draw(&mut self, text: &str) {
        if self.terminal {
            let _ = write!(self.sink, "\r{text}");
            let _ = self.sink.flush();
            self.drawn = true;
        }
    }
}

/// The stage's own percent, truncated like Python's `int(fraction * 100)`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn shown_percent(fraction: f64) -> u32 {
    (fraction * 100.0) as u32
}

/// `low + (high - low) * fraction`, truncated like Python's `int()`; `fraction` is within 0..=1.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scaled(low: u32, high: u32, fraction: f64) -> u32 {
    (f64::from(low) + f64::from(high - low) * fraction) as u32
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Capture {
        fn lines(&self) -> Vec<String> {
            let bytes = self.0.lock().unwrap().clone();
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .filter(|l| l.starts_with("ATVPREP-"))
                .map(str::to_owned)
                .collect()
        }

        fn percents(&self) -> Vec<u32> {
            self.lines()
                .iter()
                .filter_map(|l| l.strip_prefix("ATVPREP-PROGRESS "))
                .map(|n| n.parse().unwrap())
                .collect()
        }
    }

    fn plain() -> (StageReporter, Capture) {
        let out = Capture::default();
        (StageReporter::new(Box::new(out.clone()), false), out)
    }

    #[test]
    fn a_run_with_encode_maps_the_stages_onto_one_rising_percent() {
        let (mut reporter, out) = plain();
        reporter.begin_file(true);
        reporter.stage(Stage::Encode);
        reporter.update(0.5);
        reporter.update(1.0);
        reporter.stage(Stage::Finalize);
        reporter.update(0.5);
        reporter.stage(Stage::Verify);
        reporter.update(1.0);

        assert_eq!(
            out.lines(),
            [
                "ATVPREP-STAGE encode",
                "ATVPREP-PROGRESS 45",
                "ATVPREP-PROGRESS 90",
                "ATVPREP-STAGE finalize",
                "ATVPREP-PROGRESS 93",
                "ATVPREP-STAGE verify",
                "ATVPREP-PROGRESS 97",
                "ATVPREP-PROGRESS 99",
            ]
        );
    }

    #[test]
    fn without_encode_the_patch_gets_the_first_part() {
        let (mut reporter, out) = plain();
        reporter.begin_file(false);
        reporter.stage(Stage::Finalize);
        reporter.update(0.5);
        reporter.update(1.0);
        assert_eq!(
            out.lines(),
            [
                "ATVPREP-STAGE finalize",
                "ATVPREP-PROGRESS 35",
                "ATVPREP-PROGRESS 70"
            ]
        );
    }

    #[test]
    fn percent_never_goes_back_and_never_reaches_100() {
        let (mut reporter, out) = plain();
        reporter.begin_file(true);
        reporter.stage(Stage::Encode);
        for fraction in [0.5, 0.2, 5.0, -1.0, 1.0] {
            reporter.update(fraction);
        }
        reporter.stage(Stage::Verify);
        reporter.update(10.0);

        let values = out.percents();
        assert!(values.is_sorted());
        assert_eq!(values.iter().max(), Some(&99));
    }

    #[test]
    fn nan_and_infinite_fractions_never_lower_or_exceed_the_percent() {
        let (mut reporter, out) = plain();
        reporter.begin_file(true);
        reporter.stage(Stage::Encode);
        for fraction in [0.5, f64::NAN, f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
            reporter.update(fraction);
        }
        reporter.stage(Stage::Verify);
        reporter.update(f64::INFINITY);

        let values = out.percents();
        assert!(values.is_sorted());
        assert_eq!(values, [45, 90, 97, 99]);
    }

    #[test]
    fn a_new_file_starts_over() {
        let (mut reporter, out) = plain();
        for _ in 0..2 {
            reporter.begin_file(true);
            reporter.stage(Stage::Encode);
            reporter.update(0.5);
        }
        assert_eq!(
            out.lines(),
            ["ATVPREP-STAGE encode", "ATVPREP-PROGRESS 45"].repeat(2)
        );
    }

    #[test]
    fn updates_before_a_stage_are_ignored() {
        let (mut reporter, out) = plain();
        reporter.begin_file(true);
        reporter.update(0.9);
        assert_eq!(out.lines(), Vec::<String>::new());
    }

    #[test]
    fn on_a_terminal_nothing_is_printed_as_protocol_lines() {
        let out = Capture::default();
        let mut reporter = StageReporter::new(Box::new(out.clone()), true);
        reporter.begin_file(false);
        reporter.stage(Stage::Finalize);
        reporter.update(0.5);
        reporter.outcome(Outcome::Done);
        reporter.finish();
        assert_eq!(out.lines(), Vec::<String>::new());
    }

    #[test]
    fn outcome_and_source_lines_name_the_result() {
        let (mut reporter, out) = plain();
        reporter.outcome(Outcome::ReencodeNeeded);
        reporter.source(true, None);
        reporter.source(false, Some("verify failed"));
        assert_eq!(
            out.lines(),
            [
                "ATVPREP-OUTCOME reencode_needed",
                "ATVPREP-SOURCE removed",
                "ATVPREP-SOURCE kept verify failed"
            ]
        );
    }

    #[test]
    fn the_terminal_reporter_draws_a_carriage_return_line_and_no_protocol_lines() {
        let out = Capture::default();
        let mut reporter = StageReporter::new(Box::new(out.clone()), true);
        reporter.begin_file(true);
        reporter.stage(Stage::Encode);
        reporter.update(0.5);
        reporter.update(0.25);
        reporter.stage(Stage::Finalize);
        reporter.finish();
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("\rEncoding …"), "{text:?}");
        assert!(text.contains("\rEncoding 50 %"), "{text:?}");
        assert!(
            !text.contains("Encoding 25 %"),
            "a lower fraction is not drawn: {text:?}"
        );
        assert!(text.contains("\rFinalizing …"), "{text:?}");
        assert!(text.ends_with("\r\x1b[K"), "{text:?}");
        assert!(!text.contains("ATVPREP-"));
    }
}

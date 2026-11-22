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
//! ATVPREP-ENCODE need=<need> allowance=<allowance>   (after `encode_not_allowed`)
//! ATVPREP-SOURCE removed | kept <why>   (when the original was meant to go)
//! ATVPREP-PLAN <stream>: <action>       (once per stream, before ffmpeg runs)
//! ATVPREP-VERIFY passed|failed, <n> blocker(s), <n> warning(s)   (the check after the run)
//! ATVPREP-ERROR <message>               (why a file failed, on one line)
//! ```
//!
//! On a terminal it prints no protocol lines; it draws the stage and its own percent as one
//! carriage-return line on the same sink (the CLI hands it stderr).

use std::io::Write;

use crate::encode::EncodeRefusal;

/// The start of the line that says what the plan does with one stream.
pub const PLAN_MARKER: &str = "ATVPREP-PLAN ";
/// The start of the line that says how the check after the run ended.
pub const VERIFY_MARKER: &str = "ATVPREP-VERIFY ";
/// The start of the line that says why a file failed.
pub const ERROR_MARKER: &str = "ATVPREP-ERROR ";

/// Where the share of the work before the stage work ends: copying the file, as percent.
const COPY_END: u32 = 20;
/// Where the stage work ends and the check after it begins, as percent.
const WORK_END: u32 = 98;
/// Only the process ending proves the work is done.
const CEILING: u32 = 99;

/// How a variant spreads one file over 0-99 %: the copy before the patch, if there is one, and the
/// share of each stage. One description per variant keeps the two together.
struct Shape {
    /// Share of the run for copying the file before the patch, as `(start, end)` percent.
    copy: Option<(u32, u32)>,
    /// Share of the run per stage, as `(stage, start, end)` percent.
    stages: &'static [(Stage, u32, u32)],
}

/// What a prepare does to one file; the plan decides, and each variant has its own shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// ffmpeg copies every stream into a new container, then the moov patch. It reports through
    /// [`Stage::Encode`] because the protocol line `ATVPREP-STAGE encode` names the ffmpeg run, not
    /// the video encode; the manager reads that line, so it stays.
    Repack,
    /// ffmpeg encodes the video.
    Encode,
    /// No ffmpeg: the file is copied, then its tags and atoms are patched.
    Patch,
    /// No ffmpeg and no copy: the tags and atoms of the file are patched where it lies.
    InPlace,
}

impl Variant {
    const fn shape(self) -> &'static Shape {
        match self {
            // Measured (#133): ffmpeg 52-68 % of the time, the moov patch 32-47 %.
            Self::Repack => &Shape {
                copy: None,
                stages: &[
                    (Stage::Encode, 0, 55),
                    (Stage::Finalize, 55, WORK_END),
                    (Stage::Verify, WORK_END, CEILING),
                ],
            },
            // NOT MEASURED: no log of a video encode exists yet; recheck once there is one.
            Self::Encode => &Shape {
                copy: None,
                stages: &[
                    (Stage::Encode, 0, 90),
                    (Stage::Finalize, 90, WORK_END),
                    (Stage::Verify, WORK_END, CEILING),
                ],
            },
            // Measured (#133): copying 15-25 %, the tag patch the rest; the check takes a blink.
            Self::Patch => &Shape {
                copy: Some((0, COPY_END)),
                stages: &[
                    (Stage::Finalize, COPY_END, WORK_END),
                    (Stage::Verify, WORK_END, CEILING),
                ],
            },
            Self::InPlace => &Shape {
                copy: None,
                stages: &[
                    (Stage::Finalize, 0, WORK_END),
                    (Stage::Verify, WORK_END, CEILING),
                ],
            },
        }
    }
}

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
    /// „Kodierung nicht freigegeben“: the file was left as it was; not a failure.
    EncodeNotAllowed,
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
            Self::EncodeNotAllowed => "encode_not_allowed",
            Self::Planned => "planned",
            Self::Failed => "failed",
        }
    }
}

pub struct StageReporter {
    sink: Box<dyn Write + Send>,
    terminal: bool,
    variant: Variant,
    stage: Option<Stage>,
    percent: u32,
    /// The variant copies first and the copy has not reported yet: the first stage must not
    /// start at its own low mark, the copy fills the share before it.
    copy_pending: bool,
    drawn: bool,
}

impl StageReporter {
    /// A reporter writing protocol lines to `sink` unless `terminal` is set.
    #[must_use]
    pub fn new(sink: Box<dyn Write + Send>, terminal: bool) -> Self {
        Self {
            sink,
            terminal,
            variant: Variant::Repack,
            stage: None,
            percent: 0,
            copy_pending: false,
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

    /// Start a new file: percent starts over and the stage weights follow the variant.
    pub fn begin_file(&mut self, variant: Variant) {
        self.finish();
        self.variant = variant;
        self.stage = None;
        self.percent = 0;
        self.copy_pending = variant.shape().copy.is_some();
    }

    /// Enter a stage; its fraction is 0 until [`update`](Self::update) says otherwise.
    pub fn stage(&mut self, stage: Stage) {
        if self.stage == Some(stage) {
            return;
        }
        self.stage = Some(stage);
        self.draw(&format!("{} …", stage.label()));
        self.line(&format!("ATVPREP-STAGE {}", stage.as_str()));
        // A patch copies before its first stage work: the copy reports its own share, so the
        // stage does not start at its low mark yet.
        if !self.copy_pending {
            self.update(0.0);
        }
    }

    /// Report how far the current stage is (0..1); anything outside is clamped, `NaN` counts as 0.
    pub fn update(&mut self, fraction: f64) {
        let Some(stage) = self.stage else {
            return;
        };
        let Some(&(_, low, high)) = self
            .variant
            .shape()
            .stages
            .iter()
            .find(|(s, _, _)| *s == stage)
        else {
            return;
        };
        self.copy_pending = false;
        self.advance(stage, low, high, fraction);
    }

    /// Report how far the copy before a patch is (0..1); only a variant with a copy has that share.
    /// The stage stays the one it is in. Like every report it never lowers the percent.
    pub fn copy_progress(&mut self, fraction: f64) {
        let Some(stage) = self.stage else {
            return;
        };
        let Some((low, high)) = self.variant.shape().copy else {
            return;
        };
        self.copy_pending = false;
        self.advance(stage, low, high, fraction);
    }

    fn advance(&mut self, stage: Stage, low: u32, high: u32, fraction: f64) {
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

    /// Say which need met which allowance, after a refused file's outcome line.
    pub fn encode_refused(&mut self, refusal: EncodeRefusal) {
        self.line(&format!(
            "ATVPREP-ENCODE need={} allowance={}",
            refusal.need.as_str(),
            refusal.allowance.as_str()
        ));
    }

    /// Say whether the original was removed, or why it was kept.
    pub fn source(&mut self, removed: bool, reason: Option<&str>) {
        match (removed, reason) {
            (true, _) => self.line("ATVPREP-SOURCE removed"),
            (false, Some(why)) => self.line(&format!("ATVPREP-SOURCE kept {why}")),
            (false, None) => self.line("ATVPREP-SOURCE kept"),
        }
    }

    /// Say what the plan does with one stream (see `output::stream_plan`).
    pub fn plan(&mut self, stream: &str) {
        self.line(&format!("{PLAN_MARKER}{}", one_line(stream)));
    }

    /// Say how the check after the run ended.
    pub fn checked(&mut self, report: &crate::verify::VerifyReport) {
        let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
        self.line(&format!(
            "{VERIFY_MARKER}{}, {}, {}",
            if report.passed() { "passed" } else { "failed" },
            count(report.blockers().len(), "blocker"),
            count(report.warnings().len(), "warning")
        ));
    }

    /// Say why a file failed.
    pub fn error(&mut self, message: &str) {
        self.line(&format!("{ERROR_MARKER}{}", one_line(message)));
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

/// `text` on one line: line breaks become spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    use crate::encode::{EncodeAllowance, EncodeNeed};

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
            atvprep_testkit::split_progress(&self.lines()).1
        }
    }

    fn plain() -> (StageReporter, Capture) {
        let out = Capture::default();
        (StageReporter::new(Box::new(out.clone()), false), out)
    }

    #[test]
    fn a_repack_gives_the_patch_a_large_share_and_the_check_the_last_point() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Repack);
        reporter.stage(Stage::Encode);
        reporter.update(0.5);
        reporter.update(1.0);
        reporter.stage(Stage::Finalize);
        reporter.update(0.5);
        reporter.update(1.0);
        reporter.stage(Stage::Verify);
        reporter.update(1.0);

        assert_eq!(
            out.lines(),
            [
                "ATVPREP-STAGE encode",
                "ATVPREP-PROGRESS 27",
                "ATVPREP-PROGRESS 55",
                "ATVPREP-STAGE finalize",
                "ATVPREP-PROGRESS 76",
                "ATVPREP-PROGRESS 98",
                "ATVPREP-STAGE verify",
                "ATVPREP-PROGRESS 99",
            ]
        );
    }

    #[test]
    fn a_video_encode_keeps_the_unmeasured_weights() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Encode);
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
                "ATVPREP-PROGRESS 94",
                "ATVPREP-STAGE verify",
                "ATVPREP-PROGRESS 98",
                "ATVPREP-PROGRESS 99",
            ]
        );
    }

    #[test]
    fn a_patch_copies_first_then_patches_the_tags_then_verifies() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Patch);
        reporter.stage(Stage::Finalize);
        reporter.copy_progress(0.5);
        reporter.copy_progress(1.0);
        reporter.update(0.5);
        reporter.update(1.0);
        reporter.stage(Stage::Verify);
        reporter.update(1.0);
        assert_eq!(
            out.lines(),
            [
                "ATVPREP-STAGE finalize",
                "ATVPREP-PROGRESS 10",
                "ATVPREP-PROGRESS 20",
                "ATVPREP-PROGRESS 59",
                "ATVPREP-PROGRESS 98",
                "ATVPREP-STAGE verify",
                "ATVPREP-PROGRESS 99"
            ]
        );
    }

    #[test]
    fn an_in_place_patch_has_no_copy_share_and_does_not_jump() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::InPlace);
        reporter.stage(Stage::Finalize);
        reporter.copy_progress(0.5);
        reporter.update(0.5);
        reporter.update(1.0);
        reporter.stage(Stage::Verify);
        reporter.update(1.0);
        assert_eq!(out.percents(), [49, 98, 99]);
    }

    #[test]
    fn the_stage_after_a_patch_copy_starts_at_its_low_mark_only_when_nothing_was_copied() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Patch);
        reporter.stage(Stage::Finalize);
        assert_eq!(
            out.percents(),
            Vec::<u32>::new(),
            "the copy fills the share first"
        );
        reporter.update(0.0);
        assert_eq!(out.percents(), [20]);
    }

    #[test]
    fn a_copy_report_after_the_patch_began_never_lowers_the_percent() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Patch);
        reporter.stage(Stage::Finalize);
        reporter.update(0.5);
        reporter.copy_progress(1.0);
        reporter.copy_progress(f64::NAN);
        assert_eq!(out.percents(), [59]);
    }

    #[test]
    fn copy_fractions_are_clamped_and_monotonic() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Patch);
        reporter.stage(Stage::Finalize);
        for fraction in [0.0, 0.5, 0.25, f64::NAN, 3.0, -1.0] {
            reporter.copy_progress(fraction);
        }
        assert_eq!(out.percents(), [10, 20]);
    }

    #[test]
    fn an_empty_file_reports_no_copy_and_the_patch_still_starts_at_the_copy_share() {
        // `copy_with_progress` reports no block for an empty file, and `fraction(0, 0)` is 0.
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Patch);
        reporter.stage(Stage::Finalize);
        reporter.copy_progress(0.0);
        reporter.update(1.0);
        reporter.stage(Stage::Verify);
        reporter.update(1.0);
        assert_eq!(out.percents(), [98, 99]);
    }

    #[test]
    fn a_video_encode_and_a_repack_end_their_encode_stage_at_their_own_mark() {
        for (variant, mark) in [(Variant::Repack, 55), (Variant::Encode, 90)] {
            let (mut reporter, out) = plain();
            reporter.begin_file(variant);
            reporter.stage(Stage::Encode);
            reporter.update(1.0);
            assert_eq!(out.percents(), [mark], "{variant:?}");
        }
    }

    #[test]
    fn copy_progress_is_ignored_outside_a_patch() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Repack);
        reporter.stage(Stage::Finalize);
        reporter.copy_progress(0.5);
        assert_eq!(out.percents(), [55]);
    }

    #[test]
    fn percent_never_goes_back_and_never_reaches_100() {
        let (mut reporter, out) = plain();
        reporter.begin_file(Variant::Repack);
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
        reporter.begin_file(Variant::Encode);
        reporter.stage(Stage::Encode);
        for fraction in [0.5, f64::NAN, f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
            reporter.update(fraction);
        }
        reporter.stage(Stage::Verify);
        reporter.update(f64::INFINITY);

        let values = out.percents();
        assert!(values.is_sorted());
        assert_eq!(values, [45, 90, 98, 99]);
    }

    #[test]
    fn a_new_file_starts_over() {
        let (mut reporter, out) = plain();
        for _ in 0..2 {
            reporter.begin_file(Variant::Encode);
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
        reporter.begin_file(Variant::Repack);
        reporter.update(0.9);
        assert_eq!(out.lines(), Vec::<String>::new());
    }

    #[test]
    fn on_a_terminal_nothing_is_printed_as_protocol_lines() {
        let out = Capture::default();
        let mut reporter = StageReporter::new(Box::new(out.clone()), true);
        reporter.begin_file(Variant::Patch);
        reporter.stage(Stage::Finalize);
        reporter.copy_progress(0.5);
        reporter.update(0.5);
        reporter.outcome(Outcome::Done);
        reporter.finish();
        assert_eq!(out.lines(), Vec::<String>::new());
    }

    #[test]
    fn outcome_and_source_lines_name_the_result() {
        let (mut reporter, out) = plain();
        reporter.outcome(Outcome::EncodeNotAllowed);
        reporter.encode_refused(EncodeRefusal {
            need: EncodeNeed::Video,
            allowance: EncodeAllowance::Audio,
        });
        reporter.source(true, None);
        reporter.source(false, Some("verify failed"));
        assert_eq!(
            out.lines(),
            [
                "ATVPREP-OUTCOME encode_not_allowed",
                "ATVPREP-ENCODE need=video allowance=audio",
                "ATVPREP-SOURCE removed",
                "ATVPREP-SOURCE kept verify failed"
            ]
        );
    }

    #[test]
    fn the_terminal_reporter_draws_a_carriage_return_line_and_no_protocol_lines() {
        let out = Capture::default();
        let mut reporter = StageReporter::new(Box::new(out.clone()), true);
        reporter.begin_file(Variant::Repack);
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

//! The action of one claimed item: Vorbereiten, Prüfen or Analysieren.
//!
//! An [`Action`] owns what its name says: the arguments and the process call, how the process
//! result is read (terminal status, code, refusal, the report to store) and which log lines it
//! writes, in which order. The execution shell in `execution.rs` owns lease, fencing, permit and
//! the item's state transitions; it hands the action three seams:
//!
//! - [`ProcessAdapter`]: the real pinned child process, or a fixed result in tests.
//! - [`Sink`]: where step lines and tool output go (the fenced item log, or a plain log in tests).
//! - [`Shell`]: what the action needs from the shell between its phases (cancellation, the lease
//!   check, writing the report, completing a Prepare). The action never writes to the database
//!   itself.
//!
//! Adding an action means adding a variant and its arms here; the shell stays as it is.
use super::{
    execution_output::ClaimedOutput,
    prepare_log::PrepareReports,
    process::{
        PrepareMode, ProcessResult, StopReason, Stream, prepare_arguments, probe_arguments,
        run_pinned_prepare_observed, run_pinned_probe, run_pinned_verify, verify_arguments,
    },
    report_log,
};
use crate::{
    Error, Result, clock::Clock, media::StoredVerdict, settings::TargetProfile,
    state_machine::ItemStatus,
};
use atvprep_core::encode::{EncodeAllowance, EncodeNeed};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, time::Duration};

/// How long an item that may encode video and needs it (or does not know yet) can run. Encoding a
/// film takes hours; the lease keeps being renewed meanwhile. Every other item keeps the configured,
/// shorter limit.
pub const REENCODE_TIMEOUT: Duration = Duration::from_hours(4);

/// The code of a successful item whose file needs more than its Kodierfreigabe (ADR 0008).
pub const ENCODE_NOT_ALLOWED: &str = "encode_not_allowed";

/// The machine line the media CLI prints when a file needs more than its Kodierfreigabe allows.
const ENCODE_NOT_ALLOWED_LINE: &str = "ATVPREP-OUTCOME encode_not_allowed";

/// The largest report stdout an inspection or check may leave.
const REPORT_LIMIT: usize = 56 * 1024;

/// The need and allowance of a refusal, from the CLI's `ATVPREP-ENCODE` line.
pub(super) fn encode_refusal(process: &ProcessResult) -> Option<String> {
    if process.reason != StopReason::Exited || process.exit_code != Some(0) {
        return None;
    }
    // Machine lines come on stdout alone; the merged log may interleave stderr with them.
    let output = String::from_utf8_lossy(&process.stdout);
    output
        .lines()
        .any(|line| line.trim() == ENCODE_NOT_ALLOWED_LINE)
        .then(|| {
            let detail = output
                .lines()
                .find_map(|line| line.trim().strip_prefix("ATVPREP-ENCODE "))
                .unwrap_or("need=unknown allowance=unknown");
            format!("Encoding not allowed ({detail}): the file stays as it was.")
        })
}

/// The time limit of one changing item: longer only for „Audio und Video“ with a video or unknown
/// Kodierbedarf (`need` is the stored one, `None` is „unbekannt“).
#[must_use]
pub fn prepare_timeout(
    configured: Duration,
    mode: PrepareMode,
    need: Option<EncodeNeed>,
) -> Duration {
    if mode.encode == EncodeAllowance::All && matches!(need, None | Some(EncodeNeed::Video)) {
        configured.max(REENCODE_TIMEOUT)
    } else {
        configured
    }
}

/// The three things an item can do, each with what only it needs.
#[derive(Clone, Copy)]
pub(super) enum Action {
    /// Vorbereiten: change the file under its approval; `need` is the stored Kodierbedarf.
    Prepare {
        mode: PrepareMode,
        need: Option<EncodeNeed>,
    },
    /// Prüfen against the profile of this moment.
    Verify { profile: TargetProfile },
    /// Analysieren: probe the streams.
    Inspect,
}

/// Which process call an action makes.
#[derive(Clone, Copy)]
pub(super) enum Call {
    Prepare(PrepareMode),
    Probe,
    Verify(TargetProfile),
}

/// The file an action works on and the limits it works under.
pub(super) struct Subject<'a> {
    /// The path in the library, for the log.
    pub relative: &'a str,
    /// The pinned leaf and its identity, for the arguments.
    pub leaf: &'a str,
    pub identity: (i128, i128, i128),
    /// The CLI, for the `Command:` line.
    pub executable: &'a Path,
    /// The configured time limit; a Vorbereiten run may get a longer one.
    pub timeout: Duration,
    pub clock: &'a dyn Clock,
}

/// Runs the media CLI, or stands in for it.
pub(super) trait ProcessAdapter {
    /// Run `call`, reporting every output chunk with its pipe to `observe`.
    fn run(
        &mut self,
        call: Call,
        timeout: Duration,
        cancelled: &mut dyn FnMut() -> bool,
        observe: &mut dyn FnMut(Stream, &[u8]) -> Result<()>,
    ) -> Result<ProcessResult>;

    /// Whether the pinned input is still the file the process saw.
    fn input_unchanged(&self) -> bool;
}

/// Takes the item's log: step lines of the action and the tool's output.
pub(super) trait Sink {
    fn feed(&mut self, chunk: &[u8], force: bool) -> Result<()>;
    fn step(&mut self, line: &str) -> Result<()>;
}

impl Sink for ClaimedOutput<'_> {
    fn feed(&mut self, chunk: &[u8], force: bool) -> Result<()> {
        ClaimedOutput::feed(self, chunk, force)
    }
    fn step(&mut self, line: &str) -> Result<()> {
        ClaimedOutput::step(self, line)
    }
}

/// What a check or analysis leaves behind: the report to store with the tool that made it.
pub(super) struct Report {
    pub tool_name: &'static str,
    /// The version the tool reported, `unknown` when it did not say.
    pub tool_version: String,
    pub document: Value,
}

/// What an action needs from the execution shell between its phases.
pub(super) trait Shell {
    /// Polled while the process runs: whether it should be stopped.
    fn cancelled(&mut self) -> bool;
    /// Whether the manager is shutting down, which ends the item as interrupted.
    fn shutdown_requested(&self) -> bool;
    /// Whether the claim or lease was lost while the process ran.
    fn authority_lost(&mut self) -> Result<bool>;
    /// Write a report; the shell checks its authority and commits.
    fn record_report(&mut self, report: &Report) -> Result<()>;
    /// What the check stored for the file, after [`Self::record_report`].
    fn stored_verdict(&mut self) -> Result<StoredVerdict>;
    /// Bring the library in line with what Vorbereiten did to the file.
    fn complete_prepare(&mut self, process: &ProcessResult) -> Result<()>;
    /// Time since the item started running, for the `Duration:` line.
    fn elapsed(&self) -> Duration;
}

/// How an action ended: what the shell sets on the item.
pub(super) struct Outcome {
    pub process: ProcessResult,
    pub terminal: ItemStatus,
    pub code: Option<&'static str>,
    /// Why a file above its Kodierfreigabe stayed unchanged.
    pub refusal: Option<String>,
}

impl Action {
    /// The name in the first log line.
    const fn name(self) -> &'static str {
        match self {
            Self::Prepare { .. } => "prepare",
            Self::Verify { .. } => "verify",
            Self::Inspect => "inspect",
        }
    }

    /// The one place that says which exits count as the tool having done its job: a normal exit with
    /// 0, and for Prüfen also 1, which means „ran and found violations“.
    fn exit_accepted(self, process: &ProcessResult) -> bool {
        process.reason == StopReason::Exited
            && (process.exit_code == Some(0)
                || (matches!(self, Self::Verify { .. }) && process.exit_code == Some(1)))
    }

    fn call(self) -> Call {
        match self {
            Self::Prepare { mode, .. } => Call::Prepare(mode),
            Self::Verify { profile } => Call::Verify(profile),
            Self::Inspect => Call::Probe,
        }
    }

    fn arguments(self, subject: &Subject<'_>) -> Vec<String> {
        match self {
            Self::Prepare { mode, .. } => prepare_arguments(subject.leaf, subject.identity, mode),
            Self::Verify { profile } => verify_arguments(subject.leaf, subject.identity, profile),
            Self::Inspect => probe_arguments(),
        }
    }

    /// Run the action: the start and frame lines, the process, what its result says, the end.
    ///
    /// # Errors
    /// Lost authority, a changed input, an unusable report, and whatever the process, sink or
    /// shell fail with.
    pub fn execute(
        self,
        subject: &Subject<'_>,
        process: &mut dyn ProcessAdapter,
        sink: &mut dyn Sink,
        shell: &mut dyn Shell,
    ) -> Result<Outcome> {
        for line in report_log::opening(self.name(), subject.relative) {
            sink.step(&line)?;
        }
        if !matches!(self, Self::Inspect) {
            sink.step(&format!("Tool: atvprep {}", env!("CARGO_PKG_VERSION")))?;
        }
        sink.step(&report_log::command(
            subject.executable,
            &self.arguments(subject),
        ))?;
        match self {
            Self::Prepare { mode, .. } => {
                for line in report_log::prepare_frame(mode) {
                    sink.step(&line)?;
                }
            }
            Self::Verify { profile } => {
                sink.step(&format!("Target profile: {}", profile.summary()))?;
            }
            Self::Inspect => {}
        }
        let mut reports = PrepareReports::default();
        let result = match self {
            Self::Prepare { mode, need } => {
                let clock = subject.clock;
                process.run(
                    self.call(),
                    prepare_timeout(subject.timeout, mode, need),
                    &mut || shell.cancelled(),
                    &mut |stream, chunk| {
                        sink.feed(chunk, false)?;
                        for line in reports.step_lines_keeping_error(stream, chunk, clock) {
                            sink.step(&line)?;
                        }
                        Ok(())
                    },
                )?
            }
            Self::Verify { .. } | Self::Inspect => process.run(
                self.call(),
                subject.timeout,
                &mut || shell.cancelled(),
                &mut |_, _| Ok(()),
            )?,
        };
        // A check that worked leaves its report, not its output; one that failed leaves its output.
        let reported = self.exit_accepted(&result);
        match self {
            Self::Prepare { .. } => sink.feed(&[], true)?,
            _ if !reported => sink.feed(&result.output, true)?,
            _ => {}
        }
        if shell.authority_lost()? {
            return Err(Error::Conflict(
                "worker lost its execution lease".to_owned(),
            ));
        }
        match self {
            Self::Prepare { .. } => shell.complete_prepare(&result)?,
            _ if reported => self.record_and_describe(&result, process, sink, shell)?,
            _ => {}
        }
        let refusal = matches!(self, Self::Prepare { .. })
            .then(|| encode_refusal(&result))
            .flatten();
        let terminal = if shell.shutdown_requested() {
            ItemStatus::Interrupted
        } else if result.reason == StopReason::Cancelled {
            ItemStatus::Cancelled
        } else if self.exit_accepted(&result) {
            ItemStatus::Succeeded
        } else {
            ItemStatus::Failed
        };
        let code = match result.reason {
            StopReason::Exited if refusal.is_some() => Some(ENCODE_NOT_ALLOWED),
            StopReason::TimedOut => Some("timeout"),
            StopReason::Cancelled => Some("cancelled"),
            StopReason::Exited if !self.exit_accepted(&result) => Some("verification_failed"),
            StopReason::Exited => None,
        };
        // The log is written while the item still runs: its last line comes before its end.
        for line in report_log::ending(result.exit_code, shell.elapsed()) {
            sink.step(&line)?;
        }
        if let Some(closing) =
            report_log::closing(terminal, code, &result, reports.error(), refusal.as_deref())
        {
            sink.step(&closing)?;
        }
        Ok(Outcome {
            process: result,
            terminal,
            code,
            refusal,
        })
    }

    /// Read the report of a finished check or analysis, have it stored, and describe it in the log.
    fn record_and_describe(
        self,
        result: &ProcessResult,
        process: &dyn ProcessAdapter,
        sink: &mut dyn Sink,
        shell: &mut dyn Shell,
    ) -> Result<()> {
        if !process.input_unchanged() {
            return Err(Error::Invalid("input changed during inspection".to_owned()));
        }
        if result.stdout.len() > REPORT_LIMIT {
            return Err(Error::Invalid(
                "inspection report exceeds supported size".to_owned(),
            ));
        }
        let mut document: Value = serde_json::from_slice(&result.stdout)
            .map_err(|_| Error::Invalid("inspection returned an invalid report".to_owned()))?;
        // The tool's version is kept beside the report, not inside it.
        let tool_version = document
            .as_object_mut()
            .and_then(|report| report.remove("program_version"))
            .and_then(|version| version.get("version")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        let (tool_name, document) = if matches!(self, Self::Verify { .. }) {
            let first = document
                .get("reports")
                .and_then(Value::as_array)
                .and_then(|reports| reports.first())
                .cloned()
                .ok_or_else(|| Error::Invalid("verification returned no report".to_owned()))?;
            ("verify", first)
        } else {
            ("ffprobe", document)
        };
        let report = Report {
            tool_name,
            tool_version,
            document,
        };
        shell.record_report(&report)?;
        if matches!(self, Self::Inspect) {
            sink.step(&format!("Tool: ffprobe {}", report.tool_version))?;
            for line in report_log::streams(&report.document) {
                sink.step(&line)?;
            }
        } else {
            for line in report_log::rules(&report.document) {
                sink.step(&line)?;
            }
            let stored = shell.stored_verdict()?;
            for line in report_log::stored_verdict(
                &stored.state,
                stored.encode_need.as_deref(),
                stored.planned_change.as_deref(),
            ) {
                sink.step(&line)?;
            }
        }
        Ok(())
    }
}

/// The real adapter: the media CLI as a child process on the pinned input.
pub(super) struct PinnedProcess<'a> {
    pub executable: &'a Path,
    pub target: &'a atvprep_core::fd_fix_contract::FdFixTarget,
    pub environment: &'a BTreeMap<String, String>,
    pub grace: Duration,
}

impl ProcessAdapter for PinnedProcess<'_> {
    fn run(
        &mut self,
        call: Call,
        timeout: Duration,
        cancelled: &mut dyn FnMut() -> bool,
        observe: &mut dyn FnMut(Stream, &[u8]) -> Result<()>,
    ) -> Result<ProcessResult> {
        match call {
            Call::Prepare(mode) => {
                // The phase timing reports each phase on stderr, for the log.
                let mut environment = self.environment.clone();
                environment.insert(
                    atvprep_core::phase_timing::ENV_VAR.to_owned(),
                    "1".to_owned(),
                );
                run_pinned_prepare_observed(
                    self.executable,
                    self.target,
                    mode,
                    &environment,
                    timeout,
                    self.grace,
                    cancelled,
                    observe,
                )
            }
            Call::Probe => run_pinned_probe(
                self.executable,
                self.target,
                self.environment,
                timeout,
                self.grace,
                cancelled,
            ),
            Call::Verify(profile) => run_pinned_verify(
                self.executable,
                self.target,
                profile,
                self.environment,
                timeout,
                self.grace,
                cancelled,
            ),
        }
    }

    fn input_unchanged(&self) -> bool {
        self.target.revalidate().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        clock::FixedClock,
        worker::logs::{ITEM_LOG_LIMIT, ItemLog},
    };
    use serde_json::json;

    const CLOCK: FixedClock = FixedClock(1_700_000_000_000_000);

    /// A process that ends as told, after saying what it is told to say.
    struct Fixed {
        result: Option<ProcessResult>,
        stdout_chunks: Vec<(Stream, Vec<u8>)>,
        unchanged: bool,
    }
    impl Fixed {
        fn new(reason: StopReason, exit: Option<i32>, stdout: &str) -> Self {
            Self {
                result: Some(ProcessResult {
                    reason,
                    exit_code: exit,
                    output: stdout.as_bytes().to_vec(),
                    stdout: stdout.as_bytes().to_vec(),
                }),
                stdout_chunks: Vec::new(),
                unchanged: true,
            }
        }
        fn saying(mut self, stream: Stream, text: &str) -> Self {
            self.stdout_chunks.push((stream, text.as_bytes().to_vec()));
            self
        }
    }
    impl ProcessAdapter for Fixed {
        fn run(
            &mut self,
            _: Call,
            _: Duration,
            _: &mut dyn FnMut() -> bool,
            observe: &mut dyn FnMut(Stream, &[u8]) -> Result<()>,
        ) -> Result<ProcessResult> {
            for (stream, chunk) in &self.stdout_chunks {
                observe(*stream, chunk)?;
            }
            Ok(self.result.take().unwrap())
        }
        fn input_unchanged(&self) -> bool {
            self.unchanged
        }
    }

    /// The log of a test: the same bounded, stamped log an item has, without a database.
    struct Log(ItemLog);
    impl Log {
        fn new() -> Self {
            Self(ItemLog::new(ITEM_LOG_LIMIT))
        }
        fn lines(&self) -> Vec<String> {
            self.0
                .text()
                .lines()
                .map(|line| {
                    match line
                        .strip_prefix('[')
                        .and_then(|rest| rest.split_once("] "))
                    {
                        Some((_, text)) => text.to_owned(),
                        None => line.to_owned(),
                    }
                })
                .collect()
        }
    }
    impl Sink for Log {
        fn feed(&mut self, chunk: &[u8], _: bool) -> Result<()> {
            self.0.tool(chunk);
            Ok(())
        }
        fn step(&mut self, line: &str) -> Result<()> {
            self.0.step(&CLOCK, line);
            Ok(())
        }
    }

    /// The golden lines of a run, `name`.txt under `testdata/golden/worker_action`. The version of the
    /// tool is written `{version}`. `UPDATE_GOLDEN=1` rewrites the file from the run.
    fn assert_golden(name: &str, lines: &[String]) {
        let path = format!(
            "{}/../../testdata/golden/worker_action/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        // The goldens predate the chunk rule (#144): a clean verdict of it adds a line they lack.
        let actual = lines
            .iter()
            .filter(|line| line.as_str() != "Rule chunk-length: ok")
            .map(|line| line.replace(env!("CARGO_PKG_VERSION"), "{version}"))
            .map(|line| line + "\n")
            .collect::<String>();
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
            std::fs::write(&path, &actual).unwrap();
        }
        let golden = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path}"));
        assert_eq!(actual, golden, "{name}");
    }

    #[derive(Default)]
    struct TestShell {
        recorded: Vec<(String, String, Value)>,
        completed: bool,
        shutdown: bool,
        lost: bool,
    }
    impl Shell for TestShell {
        fn cancelled(&mut self) -> bool {
            false
        }
        fn shutdown_requested(&self) -> bool {
            self.shutdown
        }
        fn authority_lost(&mut self) -> Result<bool> {
            Ok(self.lost)
        }
        fn record_report(&mut self, report: &Report) -> Result<()> {
            self.recorded.push((
                report.tool_name.to_owned(),
                report.tool_version.clone(),
                report.document.clone(),
            ));
            Ok(())
        }
        fn stored_verdict(&mut self) -> Result<StoredVerdict> {
            Ok(StoredVerdict {
                state: "warning".to_owned(),
                encode_need: Some("audio".to_owned()),
                planned_change: None,
            })
        }
        fn complete_prepare(&mut self, _: &ProcessResult) -> Result<()> {
            self.completed = true;
            Ok(())
        }
        fn elapsed(&self) -> Duration {
            Duration::from_millis(4210)
        }
    }

    fn run(
        action: Action,
        mut process: Fixed,
        shell: &mut TestShell,
    ) -> (Result<Outcome>, Vec<String>) {
        let subject = Subject {
            relative: "film.mp4",
            leaf: "film.mp4",
            identity: (1, 2, 3),
            executable: Path::new("/opt/bin/atvprep"),
            timeout: Duration::from_secs(60),
            clock: &CLOCK,
        };
        let mut log = Log::new();
        let outcome = action.execute(&subject, &mut process, &mut log, shell);
        (outcome, log.lines())
    }

    const PROBE: &str = r#"{"program_version":{"version":"7.1.1","configuration":"--private"},
        "streams":[{"index":0,"codec_type":"video","codec_name":"h264","disposition":{"default":1}}]}"#;

    #[test]
    fn an_analysis_logs_its_streams_and_stores_the_tool_version_apart_from_the_report() {
        let mut shell = TestShell::default();
        let (outcome, lines) = run(
            Action::Inspect,
            Fixed::new(StopReason::Exited, Some(0), PROBE),
            &mut shell,
        );
        let outcome = outcome.unwrap();
        assert_eq!(outcome.terminal, ItemStatus::Succeeded);
        assert_eq!(outcome.code, None);
        assert_golden("inspect", &lines);
        assert_eq!(shell.recorded.len(), 1);
        assert_eq!(shell.recorded[0].0, "ffprobe");
        assert_eq!(shell.recorded[0].1, "7.1.1");
        assert!(shell.recorded[0].2.get("program_version").is_none());
    }

    const VERIFY_REPORT: &str = r#"{"reports":[{"passed":false,
        "violations":[{"rule":"fast-start","severity":"warning","message":"moov at end"}]}]}"#;

    fn verify() -> Action {
        Action::Verify {
            profile: TargetProfile::default(),
        }
    }

    #[test]
    fn a_check_with_findings_succeeds_and_logs_every_rule_and_the_stored_verdict() {
        let mut shell = TestShell::default();
        let (outcome, lines) = run(
            verify(),
            Fixed::new(StopReason::Exited, Some(1), VERIFY_REPORT),
            &mut shell,
        );
        let outcome = outcome.unwrap();
        assert_eq!(outcome.terminal, ItemStatus::Succeeded);
        assert_eq!(outcome.code, None);
        assert_golden("verify_with_findings", &lines);
        // The stored result carries the tool version when the report names none: `unknown`.
        assert_eq!(shell.recorded[0].0, "verify");
        assert_eq!(shell.recorded[0].1, "unknown");
    }

    #[test]
    fn a_check_takes_the_tool_version_of_its_report_when_it_names_one() {
        let mut shell = TestShell::default();
        let report = r#"{"program_version":{"version":"2.3.4"},"reports":[{"violations":[]}]}"#;
        let (outcome, _) = run(
            verify(),
            Fixed::new(StopReason::Exited, Some(0), report),
            &mut shell,
        );
        outcome.unwrap();
        assert_eq!(shell.recorded[0].1, "2.3.4");
    }

    #[test]
    fn a_tool_failure_leaves_its_output_and_the_cause_as_the_last_line() {
        let mut shell = TestShell::default();
        let (outcome, lines) = run(
            verify(),
            Fixed::new(StopReason::Exited, Some(2), "boom\n"),
            &mut shell,
        );
        let outcome = outcome.unwrap();
        assert_eq!(outcome.terminal, ItemStatus::Failed);
        assert_eq!(outcome.code, Some("verification_failed"));
        assert!(shell.recorded.is_empty());
        assert_golden("verify_tool_failure", &lines);
    }

    #[test]
    fn a_timeout_fails_with_its_code_and_a_cancel_ends_as_cancelled() {
        let mut shell = TestShell::default();
        let (outcome, lines) = run(
            Action::Inspect,
            Fixed::new(StopReason::TimedOut, None, ""),
            &mut shell,
        );
        let outcome = outcome.unwrap();
        assert_eq!(outcome.terminal, ItemStatus::Failed);
        assert_eq!(outcome.code, Some("timeout"));
        assert_eq!(
            lines.last().unwrap(),
            "Failed: timeout: the tool ran out of time"
        );

        let (outcome, lines) = run(
            Action::Inspect,
            Fixed::new(StopReason::Cancelled, None, ""),
            &mut shell,
        );
        let outcome = outcome.unwrap();
        assert_eq!(outcome.terminal, ItemStatus::Cancelled);
        assert_eq!(outcome.code, Some("cancelled"));
        assert_eq!(lines.last().unwrap(), "Cancelled");
    }

    #[test]
    fn a_shutdown_interrupts_the_item() {
        let mut shell = TestShell {
            shutdown: true,
            ..TestShell::default()
        };
        let (outcome, lines) = run(
            Action::Inspect,
            Fixed::new(StopReason::Cancelled, None, ""),
            &mut shell,
        );
        assert_eq!(outcome.unwrap().terminal, ItemStatus::Interrupted);
        assert_eq!(
            lines.last().unwrap(),
            "Interrupted: the manager is shutting down"
        );
    }

    #[test]
    fn a_lost_lease_is_a_conflict_and_nothing_is_stored() {
        let mut shell = TestShell {
            lost: true,
            ..TestShell::default()
        };
        let (outcome, _) = run(
            Action::Inspect,
            Fixed::new(StopReason::Exited, Some(0), PROBE),
            &mut shell,
        );
        assert!(matches!(outcome, Err(Error::Conflict(_))));
        assert!(shell.recorded.is_empty());
    }

    #[test]
    fn a_report_that_is_not_json_or_whose_input_changed_is_refused() {
        let mut shell = TestShell::default();
        let (outcome, _) = run(
            Action::Inspect,
            Fixed::new(StopReason::Exited, Some(0), "not json"),
            &mut shell,
        );
        assert!(matches!(outcome, Err(Error::Invalid(_))));
        let mut changed = Fixed::new(StopReason::Exited, Some(0), PROBE);
        changed.unchanged = false;
        let (outcome, _) = run(Action::Inspect, changed, &mut shell);
        assert!(matches!(outcome, Err(Error::Invalid(_))));
        assert!(shell.recorded.is_empty());
    }

    fn prepare(encode: EncodeAllowance) -> Action {
        Action::Prepare {
            mode: PrepareMode {
                encode,
                delete_original: true,
                force: false,
                profile: TargetProfile::default(),
            },
            need: None,
        }
    }

    #[test]
    fn a_preparation_logs_its_frame_the_tools_stages_and_completes_the_library() {
        let mut shell = TestShell::default();
        let (outcome, lines) = run(
            prepare(EncodeAllowance::Copy),
            Fixed::new(StopReason::Exited, Some(0), "")
                .saying(Stream::Stdout, "ATVPREP-SOURCE removed\n")
                .saying(
                    Stream::Stderr,
                    &format!("{}\n", json!({"phase":"remux","seconds":2.5})),
                ),
            &mut shell,
        );
        let outcome = outcome.unwrap();
        assert_eq!(outcome.terminal, ItemStatus::Succeeded);
        assert!(shell.completed);
        // The time of day a phase started is local time: the golden file does not hold it.
        let lines: Vec<String> = lines
            .iter()
            .map(|line| {
                let started = line.starts_with("Phase remux: started ");
                match line.find(", took") {
                    Some(end) if started => {
                        format!("Phase remux: started {{time}}{}", &line[end..])
                    }
                    _ => line.clone(),
                }
            })
            .collect();
        assert_golden("prepare_copy", &lines);
    }

    #[test]
    fn a_refused_encode_is_a_success_with_the_refusal_as_the_last_line() {
        let mut shell = TestShell::default();
        let (outcome, lines) = run(
            prepare(EncodeAllowance::Audio),
            Fixed::new(
                StopReason::Exited,
                Some(0),
                "ATVPREP-ENCODE need=video allowance=audio\nATVPREP-OUTCOME encode_not_allowed\n",
            ),
            &mut shell,
        );
        let outcome = outcome.unwrap();
        assert_eq!(outcome.terminal, ItemStatus::Succeeded);
        assert_eq!(outcome.code, Some(ENCODE_NOT_ALLOWED));
        assert_golden("prepare_encode_not_allowed", &lines);
    }

    #[test]
    fn a_failed_preparation_names_the_error_the_tool_reported() {
        let mut shell = TestShell::default();
        let (outcome, lines) = run(
            prepare(EncodeAllowance::Copy),
            Fixed::new(StopReason::Exited, Some(3), "")
                .saying(Stream::Stdout, "ATVPREP-ERROR disk full\n"),
            &mut shell,
        );
        assert_eq!(outcome.unwrap().terminal, ItemStatus::Failed);
        assert_golden("prepare_failed", &lines);
    }

    #[test]
    fn only_a_changing_run_over_video_may_run_longer_than_configured() {
        let mode = |encode| PrepareMode {
            encode,
            delete_original: false,
            force: false,
            profile: TargetProfile::default(),
        };
        let configured = Duration::from_secs(60);
        assert_eq!(
            prepare_timeout(configured, mode(EncodeAllowance::All), None),
            REENCODE_TIMEOUT
        );
        assert_eq!(
            prepare_timeout(configured, mode(EncodeAllowance::Copy), None),
            configured
        );
    }
}

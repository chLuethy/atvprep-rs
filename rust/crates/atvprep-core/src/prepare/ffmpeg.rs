//! Run the planned ffmpeg command and turn its `-progress` output into stage progress.

use std::fmt;
use std::io::{self, BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::models::Plan;
use crate::pipeline_postfix::RunContext;
use crate::pyfmt::repr_str;
use crate::stages::Stage;

/// Share of the duration after which ffmpeg only has the trailer left to write.
const ENCODE_DONE: f64 = 0.995;

/// Why the ffmpeg run did not produce a file.
#[derive(Debug)]
pub enum FfmpegError {
    /// The program could not be started.
    Spawn(io::Error),
    /// It ran and failed; `code` is `None` when a signal ended it (then `signal` is set).
    Failed {
        code: Option<i32>,
        signal: Option<i32>,
        cmd: Vec<String>,
    },
}

fn signal_name(signal: i32) -> String {
    let name = match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return signal.to_string(),
    };
    format!("<Signals.{name}: {signal}>")
}

impl fmt::Display for FfmpegError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "{e}"),
            Self::Failed { code, signal, cmd } => {
                let list = cmd
                    .iter()
                    .map(|a| repr_str(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                match (code, signal) {
                    (Some(code), _) => {
                        write!(
                            f,
                            "Command '[{list}]' returned non-zero exit status {code}."
                        )
                    }
                    (None, Some(signal)) => {
                        write!(f, "Command '[{list}]' died with {}.", signal_name(*signal))
                    }
                    (None, None) => write!(f, "Command '[{list}]' failed."),
                }
            }
        }
    }
}

impl std::error::Error for FfmpegError {}

/// The plan's argv with `-nostats -progress pipe:1` before the output path.
fn argv(plan: &Plan) -> Vec<String> {
    let mut cmd = plan.ffmpeg_cmd.clone();
    if let Some(output) = cmd.pop() {
        cmd.extend([
            "-nostats".to_owned(),
            "-progress".to_owned(),
            "pipe:1".to_owned(),
        ]);
        cmd.push(output);
    }
    cmd
}

/// The seconds a `-progress` line reports, if it is a usable time line.
fn seconds(line: &str) -> Option<f64> {
    let (key, value) = line.split_once('=')?;
    if key != "out_time_ms" && key != "out_time_us" {
        return None;
    }
    let micros: i64 = value.trim().parse().ok()?;
    #[allow(clippy::cast_precision_loss)]
    (micros > 0).then(|| micros as f64 / 1_000_000.0)
}

/// Run ffmpeg on the plan; the reporter shows the encode and then the finalize stage.
///
/// # Errors
///
/// [`FfmpegError::Spawn`] when ffmpeg cannot be started, [`FfmpegError::Failed`] when it exits
/// non-zero or dies from a signal.
pub fn run_ffmpeg_with_progress(ctx: &mut RunContext, plan: &Plan) -> Result<(), FfmpegError> {
    let cmd = argv(plan);
    let (planned, args) = cmd.split_first().ok_or_else(|| {
        FfmpegError::Spawn(io::Error::new(io::ErrorKind::InvalidInput, "empty command"))
    })?;
    let program = ctx
        .ffmpeg
        .as_deref()
        .map_or_else(|| Path::new(planned), |p| p);
    let total = plan.duration_s.filter(|d| *d > 0.0);
    ctx.reporter.begin_file(true);
    ctx.reporter.stage(Stage::Encode);
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(FfmpegError::Spawn)?;
    let mut finalizing = false;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line);
            let (Some(total), Some(secs)) = (total, seconds(&line)) else {
                continue;
            };
            if finalizing {
                continue;
            }
            let fraction = secs.min(total) / total;
            if fraction >= ENCODE_DONE {
                finalizing = true;
                ctx.reporter.update(1.0);
                ctx.reporter.stage(Stage::Finalize);
            } else {
                ctx.reporter.update(fraction);
            }
        }
    }
    let status = child.wait().map_err(FfmpegError::Spawn)?;
    if status.success() {
        return Ok(());
    }
    Err(FfmpegError::Failed {
        code: status.code(),
        signal: status.signal(),
        cmd,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::compliance::tools::SystemTools;
    use crate::console::{Console, SharedBuffer};
    use crate::ffprobe::Ffprobe;
    use crate::stages::StageReporter;

    fn script(dir: &Path, body: &str) -> String {
        let path = dir.join("ffmpeg");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn plan(program: &str, duration: Option<f64>) -> Plan {
        Plan {
            video_info: None,
            audio_tracks: Vec::new(),
            sub_tracks: Vec::new(),
            ffmpeg_cmd: ["-i", "in.mkv", "out.mp4"]
                .iter()
                .map(|s| (*s).to_owned())
                .fold(vec![program.to_owned()], |mut v, s| {
                    v.push(s);
                    v
                }),
            warnings: Vec::new(),
            in_path: PathBuf::from("in.mkv"),
            out_path: PathBuf::from("out.mp4"),
            duration_s: duration,
        }
    }

    fn run(program: &str, duration: Option<f64>) -> (Result<(), FfmpegError>, Vec<String>) {
        let machine = SharedBuffer::default();
        let mut ctx = RunContext {
            ffmpeg: None,
            console: Console::recording(false).0,
            reporter: StageReporter::new(Box::new(machine.clone()), false),
            tools: Box::new(SystemTools::new()),
            ffprobe: Ffprobe::new(),
        };
        let result = run_ffmpeg_with_progress(&mut ctx, &plan(program, duration));
        (result, machine.lines())
    }

    #[test]
    fn progress_lines_become_protocol_lines() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = script(
            dir.path(),
            "for t in 2000000 5000000 12000000; do echo out_time_ms=$t; done",
        );
        let (result, lines) = run(&ffmpeg, Some(10.0));
        result.unwrap();
        assert_eq!(
            lines,
            [
                "ATVPREP-STAGE encode",
                "ATVPREP-PROGRESS 18",
                "ATVPREP-PROGRESS 45",
                "ATVPREP-PROGRESS 90",
                "ATVPREP-STAGE finalize",
            ]
        );
    }

    #[test]
    fn garbage_and_negative_progress_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = script(
            dir.path(),
            "echo out_time_ms=-5; echo out_time_ms=abc; echo nonsense; echo out_time_ms=0; echo frame=3",
        );
        let (result, lines) = run(&ffmpeg, Some(10.0));
        result.unwrap();
        assert_eq!(lines, ["ATVPREP-STAGE encode"]);
    }

    #[test]
    fn unknown_duration_never_reports_a_fraction() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = script(dir.path(), "echo out_time_ms=5000000");
        for duration in [None, Some(0.0)] {
            let (result, lines) = run(&ffmpeg, duration);
            result.unwrap();
            assert_eq!(lines, ["ATVPREP-STAGE encode"]);
        }
    }

    #[test]
    fn a_failing_ffmpeg_is_an_error_with_its_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = script(dir.path(), "exit 3");
        let (result, _) = run(&ffmpeg, None);
        let error = result.unwrap_err();
        assert!(matches!(error, FfmpegError::Failed { code: Some(3), .. }));
        assert!(error.to_string().starts_with("Command '['"));
        assert!(
            error
                .to_string()
                .ends_with("', 'out.mp4']' returned non-zero exit status 3.")
        );
    }

    #[test]
    fn a_signal_death_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = script(dir.path(), "kill -KILL $$");
        let (result, _) = run(&ffmpeg, None);
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            FfmpegError::Failed {
                code: None,
                signal: Some(9),
                ..
            }
        ));
        assert!(
            error
                .to_string()
                .ends_with("died with <Signals.SIGKILL: 9>.")
        );
    }

    #[test]
    fn a_missing_ffmpeg_is_a_spawn_error() {
        let (result, _) = run("/nonexistent/ffmpeg", None);
        assert!(matches!(result, Err(FfmpegError::Spawn(_))));
    }

    #[test]
    fn the_argv_gets_progress_before_the_output_path() {
        let dir = tempfile::tempdir().unwrap();
        let echoed = dir.path().join("argv");
        let ffmpeg = script(dir.path(), &format!("echo \"$@\" > '{}'", echoed.display()));
        let (result, _) = run(&ffmpeg, None);
        result.unwrap();
        assert_eq!(
            std::fs::read_to_string(echoed).unwrap().trim(),
            "-i in.mkv -nostats -progress pipe:1 out.mp4"
        );
    }
}

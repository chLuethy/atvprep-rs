//! Bounded subprocess execution with process-group cancellation.
use super::executor::validate_executable;
use crate::{Error, Result};
use atvprep_core::encode::EncodeAllowance;
use std::os::unix::process::CommandExt;
use std::{
    collections::BTreeMap,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Why a child stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Exited,
    Cancelled,
    TimedOut,
}

/// Bounded, raw output; callers redact before writing it to logs or events.
pub struct ProcessResult {
    pub reason: StopReason,
    pub exit_code: Option<i32>,
    pub output: Vec<u8>,
    /// The child's stdout alone, bounded like `output`; stderr warnings must not corrupt a JSON report.
    pub stdout: Vec<u8>,
}

fn nonblocking(fd: impl std::os::fd::AsFd) -> Result<()> {
    let flags = rustix::fs::fcntl_getfl(&fd)
        .map_err(|_| Error::Invalid("cannot configure child output".to_owned()))?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|_| Error::Invalid("cannot configure child output".to_owned()))
}

fn drain(
    reader: &mut impl Read,
    tail: &mut Vec<u8>,
    mut only: Option<&mut Vec<u8>>,
    observed: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut buffer = [0u8; 8192];
    // Bound work per pass so a noisy child cannot prevent cancellation checks.
    for _ in 0..32 {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                observed(&buffer[..count])?;
                for buffer_tail in std::iter::once(&mut *tail).chain(only.as_deref_mut()) {
                    buffer_tail.extend_from_slice(&buffer[..count]);
                    if buffer_tail.len() > 64 * 1024 {
                        buffer_tail.drain(..buffer_tail.len() - 64 * 1024);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(Error::Invalid("cannot read child output".to_owned())),
        }
    }
    Ok(())
}

/// Run a trusted argument vector without a shell and with an explicit environment.
///
/// # Errors
/// Rejects unsafe executables and reports sanitized process/pipe failures.
pub fn run(
    executable: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    timeout: Duration,
    grace: Duration,
    cancelled: impl FnMut() -> bool,
) -> Result<ProcessResult> {
    run_observed(
        executable,
        arguments,
        environment,
        timeout,
        grace,
        cancelled,
        |_| Ok(()),
    )
}

/// Observe bounded output chunks while the child is still running.
///
/// # Errors
/// An observer failure terminates and reaps the process group before returning.
pub fn run_observed(
    executable: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    timeout: Duration,
    grace: Duration,
    cancelled: impl FnMut() -> bool,
    mut observed: impl FnMut(&[u8]) -> Result<()>,
) -> Result<ProcessResult> {
    run_with_stdin(
        executable,
        arguments,
        environment,
        timeout,
        grace,
        Stdio::null(),
        cancelled,
        &mut observed,
    )
}

/// How an authorized item may run, as decided by its approval (ADR 0008).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareMode {
    /// The item's signed Kodierfreigabe, passed to the CLI as `--encode`.
    pub encode: EncodeAllowance,
    /// The signed `delete_original` setting.
    pub delete_original: bool,
}

/// The argument vector of `atvprep prepare` for one pinned leaf.
fn prepare_arguments(leaf: &str, identity: (i128, i128, i128), mode: PrepareMode) -> Vec<String> {
    let (device, inode, ctime) = identity;
    let mut arguments = vec![
        "prepare".to_owned(),
        "--yes".to_owned(),
        "--encode".to_owned(),
        mode.encode.as_str().to_owned(),
    ];
    // As before this ticket: only an item released for video may remove its original (#56).
    if mode.encode == EncodeAllowance::All && mode.delete_original {
        arguments.push("--delete-original".to_owned());
    }
    arguments.extend([
        "--manager-parent-fd".to_owned(),
        "0".to_owned(),
        "--manager-leaf".to_owned(),
        leaf.to_owned(),
        "--manager-device".to_owned(),
        device.to_string(),
        "--manager-inode".to_owned(),
        inode.to_string(),
        "--manager-ctime-ns".to_owned(),
        ctime.to_string(),
        "--".to_owned(),
        leaf.to_owned(),
    ]);
    arguments
}

/// Execute Prepare using a pinned directory mapped to the child's descriptor zero.
///
/// # Errors
/// Rejects changed file identities and failed descriptor handoff or execution.
pub fn run_pinned_prepare(
    executable: &Path,
    target: &atvprep_core::fd_fix_contract::FdFixTarget,
    mode: PrepareMode,
    environment: &BTreeMap<String, String>,
    timeout: Duration,
    grace: Duration,
    cancelled: impl FnMut() -> bool,
) -> Result<ProcessResult> {
    run_pinned_prepare_observed(
        executable,
        target,
        mode,
        environment,
        timeout,
        grace,
        cancelled,
        |_| Ok(()),
    )
}

/// Execute pinned Prepare with incremental output observation.
///
/// # Errors
/// Rejects unsafe input identities, process failures and observer failures.
#[allow(clippy::too_many_arguments)] // The approved mode joins the explicit process policy at the spawn boundary.
pub fn run_pinned_prepare_observed(
    executable: &Path,
    target: &atvprep_core::fd_fix_contract::FdFixTarget,
    mode: PrepareMode,
    environment: &BTreeMap<String, String>,
    timeout: Duration,
    grace: Duration,
    cancelled: impl FnMut() -> bool,
    mut observed: impl FnMut(&[u8]) -> Result<()>,
) -> Result<ProcessResult> {
    target
        .revalidate()
        .map_err(|_| Error::Invalid("approved input changed".to_owned()))?;
    let directory = rustix::fs::open(
        format!("/dev/fd/{}", target.parent_fd_number()),
        // macOS rejects O_DIRECTORY on /dev/fd; the target owns a pinned directory.
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| Error::Invalid("cannot hand off media directory".to_owned()))?;
    let arguments = prepare_arguments(target.leaf_name(), target.identity(), mode);
    run_with_stdin(
        executable,
        &arguments,
        environment,
        timeout,
        grace,
        Stdio::from(std::fs::File::from(directory)),
        cancelled,
        &mut observed,
    )
}

#[allow(clippy::too_many_arguments)] // Process input/output policies stay explicit at the spawn boundary.
fn run_with_stdin(
    executable: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    timeout: Duration,
    grace: Duration,
    stdin: Stdio,
    mut cancelled: impl FnMut() -> bool,
    observed: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<ProcessResult> {
    validate_executable(executable)?;
    let mut child = Command::new(executable)
        .args(arguments)
        .env_clear()
        .envs(environment)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|_| Error::Invalid("cannot start media process".to_owned()))?;
    let group = rustix::process::Pid::from_raw(
        i32::try_from(child.id())
            .map_err(|_| Error::Invalid("invalid process identity".to_owned()))?,
    )
    .ok_or_else(|| Error::Invalid("invalid process identity".to_owned()))?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Invalid("missing child output".to_owned()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Invalid("missing child output".to_owned()))?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let started = Instant::now();
        let mut stopping = None;
        let mut reason = StopReason::Exited;
        let mut output = Vec::new();
        let mut only_stdout = Vec::new();
        loop {
            drain(&mut stdout, &mut output, Some(&mut only_stdout), observed)?;
            drain(&mut stderr, &mut output, None, observed)?;
            if stopping.is_none() {
                if cancelled() {
                    reason = StopReason::Cancelled;
                } else if started.elapsed() >= timeout {
                    reason = StopReason::TimedOut;
                }
                if reason != StopReason::Exited {
                    let _ =
                        rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
                    stopping = Some(Instant::now());
                }
            }
            if stopping.is_some_and(|time| time.elapsed() >= grace) {
                let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                let status = child
                    .wait()
                    .map_err(|_| Error::Invalid("cannot reap media process".to_owned()))?;
                drain(&mut stdout, &mut output, Some(&mut only_stdout), observed)?;
                drain(&mut stderr, &mut output, None, observed)?;
                return Ok(ProcessResult {
                    reason,
                    exit_code: status.code(),
                    output,
                    stdout: only_stdout,
                });
            }
            if stopping.is_none()
                && let Some(status) = child
                    .try_wait()
                    .map_err(|_| Error::Invalid("cannot inspect media process".to_owned()))?
            {
                drain(&mut stdout, &mut output, Some(&mut only_stdout), observed)?;
                drain(&mut stderr, &mut output, None, observed)?;
                return Ok(ProcessResult {
                    reason,
                    exit_code: status.code(),
                    output,
                    stdout: only_stdout,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    if result.is_err() {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        let _ = child.wait();
    }
    result
}

/// Probe a pinned regular file with ffprobe through a read-only stdin descriptor.
///
/// # Errors
/// Rejects replaced input and process or descriptor failures.
pub fn run_pinned_probe(
    executable: &Path,
    target: &atvprep_core::fd_fix_contract::FdFixTarget,
    environment: &BTreeMap<String, String>,
    timeout: Duration,
    grace: Duration,
    cancelled: impl FnMut() -> bool,
) -> Result<ProcessResult> {
    let file = target
        .open_for_tools()
        .map_err(|_| Error::Invalid("probe input changed".to_owned()))?;
    let arguments = [
        "-v",
        "error",
        "-show_format",
        "-show_streams",
        "-of",
        "json",
        "/dev/fd/0",
    ]
    .map(str::to_owned);
    run_with_stdin(
        executable,
        &arguments,
        environment,
        timeout,
        grace,
        Stdio::from(std::fs::File::from(file)),
        cancelled,
        &mut |_| Ok(()),
    )
}

/// Verify a pinned library file without exposing its configured root to the child.
///
/// # Errors
/// Rejects stale inputs and failed descriptor handoff or process execution.
pub fn run_pinned_verify(
    executable: &Path,
    target: &atvprep_core::fd_fix_contract::FdFixTarget,
    environment: &BTreeMap<String, String>,
    timeout: Duration,
    grace: Duration,
    cancelled: impl FnMut() -> bool,
) -> Result<ProcessResult> {
    target
        .revalidate()
        .map_err(|_| Error::Invalid("verification input changed".to_owned()))?;
    let directory = rustix::fs::open(
        format!("/dev/fd/{}", target.parent_fd_number()),
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| Error::Invalid("cannot hand off media directory".to_owned()))?;
    let (device, inode, ctime) = target.identity();
    let arguments = vec![
        "verify".to_owned(),
        "--json".to_owned(),
        "--manager-parent-fd".to_owned(),
        "0".to_owned(),
        "--manager-leaf".to_owned(),
        target.leaf_name().to_owned(),
        "--manager-device".to_owned(),
        device.to_string(),
        "--manager-inode".to_owned(),
        inode.to_string(),
        "--manager-ctime-ns".to_owned(),
        ctime.to_string(),
        "--".to_owned(),
        target.leaf_name().to_owned(),
    ];
    run_with_stdin(
        executable,
        &arguments,
        environment,
        timeout,
        grace,
        Stdio::from(std::fs::File::from(directory)),
        cancelled,
        &mut |_| Ok(()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_starts_with_the_items_own_level_and_only_all_may_delete() {
        fn level(arguments: &[String]) -> &str {
            let at = arguments.iter().position(|a| a == "--encode").unwrap();
            &arguments[at + 1]
        }
        let args = |encode, delete_original| {
            prepare_arguments(
                "a.mp4",
                (1, 2, 3),
                PrepareMode {
                    encode,
                    delete_original,
                },
            )
        };
        for (encode, name) in [
            (EncodeAllowance::Copy, "copy"),
            (EncodeAllowance::Audio, "audio"),
            (EncodeAllowance::All, "all"),
        ] {
            assert_eq!(level(&args(encode, false)), name);
            assert!(!args(encode, false).contains(&"--delete-original".to_owned()));
        }
        assert!(!args(EncodeAllowance::Audio, true).contains(&"--delete-original".to_owned()));
        assert!(args(EncodeAllowance::All, true).contains(&"--delete-original".to_owned()));
        // The leaf and the pinned identity always travel the same way.
        let arguments = args(EncodeAllowance::All, true);
        assert_eq!(arguments[0], "prepare");
        assert_eq!(arguments.last().unwrap(), "a.mp4");
        assert!(
            arguments
                .windows(2)
                .any(|w| w[0] == "--manager-inode" && w[1] == "2")
        );
    }

    fn tool(name: &str, arguments: &[&str]) -> (std::path::PathBuf, Vec<String>) {
        let executable = Path::new("/bin").join(name).canonicalize().unwrap();
        let mut args = Vec::new();
        if executable.file_name().is_some_and(|leaf| leaf == "busybox") {
            args.push(name.to_owned());
        }
        args.extend(arguments.iter().map(|value| (*value).to_owned()));
        (executable, args)
    }

    #[test]
    fn output_observer_unblocks_a_child_before_it_exits() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("producer");
        let marker = temp.path().join("continue");
        std::fs::write(&script,b"#!/bin/sh\nprintf 'ATVPREP-PROGRESS 25\\n'\nwhile [ ! -f \"$CONTINUE_MARKER\" ]; do sleep 0.01; done\nprintf 'done\\n'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let environment = BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            (
                "CONTINUE_MARKER".to_owned(),
                marker.to_string_lossy().into_owned(),
            ),
        ]);
        let mut seen = false;
        let result = run_observed(
            &script,
            &[],
            &environment,
            Duration::from_secs(3),
            Duration::from_millis(20),
            || false,
            |chunk| {
                if String::from_utf8_lossy(chunk).contains("ATVPREP-PROGRESS") {
                    seen = true;
                    std::fs::write(&marker, []).unwrap();
                }
                Ok(())
            },
        )
        .unwrap();
        assert!(seen);
        assert_eq!(result.reason, StopReason::Exited);
        assert_eq!(result.exit_code, Some(0));
        let before = Instant::now();
        let error = run_observed(
            &script,
            &[],
            &environment,
            Duration::from_secs(3),
            Duration::from_millis(20),
            || false,
            |_| Err(Error::Invalid("observer refused output".to_owned())),
        );
        assert!(error.is_err());
        assert!(before.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn captures_output_and_reaps_a_timed_out_process() {
        let (echo, arguments) = tool("echo", &["processed"]);
        let output = run(
            &echo,
            &arguments,
            &BTreeMap::new(),
            Duration::from_secs(2),
            Duration::from_millis(20),
            || false,
        )
        .unwrap();
        assert_eq!(output.reason, StopReason::Exited);
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.output, b"processed\n");
        assert_eq!(output.stdout, b"processed\n");
        // stderr is logged but never part of the stdout a JSON report is parsed from.
        let (shell, arguments) = tool("sh", &["-c", "echo warning >&2; echo report"]);
        let split = run(
            &shell,
            &arguments,
            &BTreeMap::new(),
            Duration::from_secs(2),
            Duration::from_millis(20),
            || false,
        )
        .unwrap();
        assert_eq!(split.stdout, b"report\n");
        assert!(split.output.windows(7).any(|window| window == b"warning"));
        let (sleep, arguments) = tool("sleep", &["10"]);
        let started = Instant::now();
        let timeout = run(
            &sleep,
            &arguments,
            &BTreeMap::new(),
            Duration::from_millis(20),
            Duration::from_millis(20),
            || false,
        )
        .unwrap();
        assert_eq!(timeout.reason, StopReason::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
        let cancellation = run(
            &sleep,
            &arguments,
            &BTreeMap::new(),
            Duration::from_secs(20),
            Duration::from_millis(20),
            || true,
        )
        .unwrap();
        assert_eq!(cancellation.reason, StopReason::Cancelled);
    }
}

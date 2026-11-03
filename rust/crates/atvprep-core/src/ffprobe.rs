//! Running `ffprobe` (port of the process-spawning half of `atvprep/probe.py`).
//!
//! The commands are exactly the ones the Python version runs. Every helper that Python lets fail
//! quietly (first DTS, header-only `hvcC`) returns `None`/`false` here too.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// An `hvcC` without a single parameter-set array is 23 bytes long.
const HVCC_HEADER_BYTES: u64 = 23;

/// Why ffprobe produced no JSON.
#[derive(Debug)]
pub enum ProbeError {
    Spawn(io::Error),
    Failed(Option<i32>),
    InvalidJson { file: String, detail: String },
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "ffprobe could not be started: {e}"),
            Self::Failed(Some(code)) => write!(f, "ffprobe exited with status {code}"),
            Self::Failed(None) => write!(f, "ffprobe was terminated by a signal"),
            Self::InvalidJson { file, detail } => {
                write!(
                    f,
                    "ffprobe lieferte kein gültiges JSON für '{file}': {detail}"
                )
            }
        }
    }
}

impl std::error::Error for ProbeError {}

/// What the planner asks of ffprobe beyond the JSON it is handed.
pub trait Prober {
    /// True when the first video stream's decoder configuration holds no VPS/SPS/PPS.
    fn hevc_has_no_parameter_sets(&self, infile: &Path) -> bool;
}

#[derive(Debug, Clone)]
pub struct Ffprobe {
    pub(crate) program: PathBuf,
    pub(crate) dts_timeout: Duration,
}

impl Default for Ffprobe {
    fn default() -> Self {
        Self::new()
    }
}

impl Ffprobe {
    #[must_use]
    pub fn new() -> Self {
        Self::with_program("ffprobe")
    }

    #[must_use]
    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            dts_timeout: Duration::from_secs(10),
        }
    }

    /// Run ffprobe and return its stdout; `None` when `timeout` ran out (the child is killed).
    fn capture(
        &self,
        args: &[&OsStr],
        timeout: Option<Duration>,
    ) -> io::Result<Option<(ExitStatus, Vec<u8>)>> {
        let mut child = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let (sender, receiver) = mpsc::channel();
        // Drain concurrently so a full pipe cannot stall the child; not joined on timeout, since a
        // grandchild may keep the pipe open.
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout.read_to_end(&mut bytes).map(|_| bytes);
            let _ = sender.send(result);
        });
        let status = match timeout {
            None => child.wait()?,
            Some(limit) => {
                let deadline = Instant::now() + limit;
                loop {
                    if let Some(status) = child.try_wait()? {
                        break status;
                    }
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(None);
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let bytes = receiver.recv().map_err(io::Error::other)??;
        Ok(Some((status, bytes)))
    }

    /// `ffprobe -show_format -show_streams` as JSON.
    ///
    /// # Errors
    ///
    /// [`ProbeError`] when ffprobe cannot run, fails, or prints something that is not JSON.
    pub fn probe_json(&self, infile: &Path) -> Result<Value, ProbeError> {
        let args: [&OsStr; 8] = [
            "-hide_banner".as_ref(),
            "-v".as_ref(),
            "error".as_ref(),
            "-print_format".as_ref(),
            "json".as_ref(),
            "-show_format".as_ref(),
            "-show_streams".as_ref(),
            infile.as_os_str(),
        ];
        let Some((status, bytes)) = self.capture(&args, None).map_err(ProbeError::Spawn)? else {
            return Err(ProbeError::Spawn(io::Error::other(
                "ffprobe timed out without a limit",
            )));
        };
        if !status.success() {
            return Err(ProbeError::Failed(status.code()));
        }
        serde_json::from_slice(&bytes).map_err(|e| ProbeError::InvalidJson {
            file: infile
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            detail: e.to_string(),
        })
    }

    /// DTS of the first video packet, or `None` when unavailable.
    ///
    /// Reads exactly one packet at the file head. Used to detect negative initial DTS that `AVPlayer`
    /// on tvOS mishandles together with MP4 edit lists.
    #[must_use]
    pub fn first_video_dts(&self, infile: &Path) -> Option<i64> {
        let args: [&OsStr; 12] = [
            "-hide_banner".as_ref(),
            "-v".as_ref(),
            "error".as_ref(),
            "-select_streams".as_ref(),
            "v:0".as_ref(),
            "-show_entries".as_ref(),
            "packet=dts".as_ref(),
            "-read_intervals".as_ref(),
            "%+#1".as_ref(),
            "-print_format".as_ref(),
            "json".as_ref(),
            infile.as_os_str(),
        ];
        let (status, bytes) = self.capture(&args, Some(self.dts_timeout)).ok()??;
        if !status.success() {
            return None;
        }
        let data: Value = serde_json::from_slice(&bytes).ok()?;
        match data.get("packets")?.as_array()?.first()?.get("dts")? {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
}

impl Prober for Ffprobe {
    fn hevc_has_no_parameter_sets(&self, infile: &Path) -> bool {
        let args: [&OsStr; 9] = [
            "-v".as_ref(),
            "error".as_ref(),
            "-select_streams".as_ref(),
            "v:0".as_ref(),
            "-show_entries".as_ref(),
            "stream=extradata_size".as_ref(),
            "-of".as_ref(),
            "csv=p=0".as_ref(),
            infile.as_os_str(),
        ];
        let Ok(Some((status, bytes))) = self.capture(&args, None) else {
            return false;
        };
        if !status.success() {
            return false;
        }
        let text = String::from_utf8_lossy(&bytes);
        let size = text
            .trim()
            .split(',')
            .next()
            .and_then(|n| n.trim().parse::<u64>().ok());
        size.is_some_and(|n| 0 < n && n <= HVCC_HEADER_BYTES)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::*;

    /// A fake `ffprobe`: a shell script with the given body, in its own temp dir.
    fn fake(body: &str) -> (tempfile::TempDir, Ffprobe) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("ffprobe");
        fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, Ffprobe::with_program(script))
    }

    #[test]
    fn probe_json_parses_stdout_and_passes_the_python_argv() {
        let (_dir, ffprobe) = fake(r#"printf '{"argv": "%s"}' "$*""#);
        let value = ffprobe.probe_json(Path::new("/m/film.mkv")).unwrap();
        assert_eq!(
            value["argv"],
            "-hide_banner -v error -print_format json -show_format -show_streams /m/film.mkv"
        );
    }

    #[test]
    fn probe_json_reports_a_non_zero_exit() {
        let (_dir, ffprobe) = fake("echo '{}'; exit 1");
        assert!(matches!(
            ffprobe.probe_json(Path::new("x")),
            Err(ProbeError::Failed(Some(1)))
        ));
    }

    #[test]
    fn probe_json_reports_invalid_json_with_the_file_name() {
        let (_dir, ffprobe) = fake("echo 'not json'");
        let error = ffprobe.probe_json(Path::new("/m/film.mkv")).unwrap_err();
        assert!(matches!(error, ProbeError::InvalidJson { .. }));
        assert!(
            error
                .to_string()
                .starts_with("ffprobe lieferte kein gültiges JSON für 'film.mkv': ")
        );
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let ffprobe = Ffprobe::with_program("/definitely/not/ffprobe");
        assert!(matches!(
            ffprobe.probe_json(Path::new("x")),
            Err(ProbeError::Spawn(_))
        ));
    }

    #[test]
    fn dts_is_read_from_the_first_packet_and_the_argv_matches_python() {
        let (_dir, ffprobe) = fake(
            r#"[ "$*" = "-hide_banner -v error -select_streams v:0 -show_entries packet=dts -read_intervals %+#1 -print_format json x.mkv" ] || exit 9
printf '{"packets": [{"dts": "-1001"}, {"dts": "5"}]}'"#,
        );
        assert_eq!(ffprobe.first_video_dts(Path::new("x.mkv")), Some(-1001));
        let (_dir, ffprobe) = fake(r#"printf '{"packets": [{"dts": 42}]}'"#);
        assert_eq!(ffprobe.first_video_dts(Path::new("x.mkv")), Some(42));
    }

    #[test]
    fn dts_is_none_for_na_empty_packets_failure_or_invalid_json() {
        for body in [
            r#"printf '{"packets": [{"dts": "N/A"}]}'"#,
            r#"printf '{"packets": [{}]}'"#,
            r#"printf '{"packets": []}'"#,
            r"printf '{}'",
            r#"printf '{"packets": [{"dts": "abc"}]}'"#,
            "printf 'garbage'",
            "exit 3",
        ] {
            let (_dir, ffprobe) = fake(body);
            assert_eq!(ffprobe.first_video_dts(Path::new("x.mkv")), None, "{body}");
        }
        assert_eq!(
            Ffprobe::with_program("/definitely/not/ffprobe").first_video_dts(Path::new("x")),
            None
        );
    }

    #[test]
    fn dts_is_none_after_the_timeout_and_does_not_wait_for_the_child() {
        let (_dir, mut ffprobe) = fake("exec sleep 5");
        ffprobe.dts_timeout = Duration::from_millis(200);
        let started = Instant::now();
        assert_eq!(ffprobe.first_video_dts(Path::new("x.mkv")), None);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn hevc_with_a_header_only_hvcc_has_no_parameter_sets() {
        let cases = [
            ("0", false),
            ("1", true),
            ("23", true),
            ("24", false),
            ("p,q", false),
            ("", false),
            ("23,5", true),
        ];
        for (output, expected) in cases {
            let guard = r#"[ "$*" = "-v error -select_streams v:0 -show_entries stream=extradata_size -of csv=p=0 x.mkv" ] || exit 9"#;
            let (_dir, ffprobe) = fake(&format!("{guard}\nprintf '{output}\\n'"));
            assert_eq!(
                ffprobe.hevc_has_no_parameter_sets(Path::new("x.mkv")),
                expected,
                "{output}"
            );
        }
        let (_dir, ffprobe) = fake("exit 1");
        assert!(!ffprobe.hevc_has_no_parameter_sets(Path::new("x.mkv")));
    }

    #[test]
    fn the_prober_trait_is_implemented_by_ffprobe() {
        fn takes(prober: &dyn Prober) -> bool {
            prober.hevc_has_no_parameter_sets(&PathBuf::from("x.mkv"))
        }
        let (_dir, ffprobe) = fake("printf '10\\n'");
        assert!(takes(&ffprobe));
    }
}

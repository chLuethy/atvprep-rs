//! Test support: loads the golden files the Python oracle writes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde::de::DeserializeOwned;

/// One oracle case: what went in and what the Python implementation returned.
#[derive(Debug, Deserialize)]
pub struct GoldenCase<I, E> {
    pub name: String,
    pub input: I,
    pub expected: E,
}

#[derive(Deserialize)]
struct GoldenFile<I, E> {
    cases: Vec<GoldenCase<I, E>>,
}

/// What the unchanged Python version saw when it ran the external tools on a fixture.
#[derive(Debug, Clone, Deserialize)]
pub struct RecordedTools {
    /// Standard output of `mp4info`; `None` when the tool failed.
    pub mp4info: Option<String>,
    /// Standard output of `mp4dump`; `None` when the tool failed.
    pub mp4dump: Option<String>,
    /// The `ffprobe` JSON; `None` when ffprobe failed.
    pub ffprobe: Option<serde_json::Value>,
    /// `duplicate_audio_indices` of the probe.
    pub duplicates: Vec<i64>,
}

#[derive(Deserialize)]
struct Named {
    file: String,
}

/// The recorded tool outputs of one fixture (`compliance_tools` golden).
///
/// # Panics
///
/// Panics when the fixture has no recording; this is test tooling.
#[must_use]
pub fn load_recorded(file: &str) -> RecordedTools {
    load_golden::<Named, RecordedTools>("compliance_tools")
        .into_iter()
        .find(|case| case.input.file == file)
        .unwrap_or_else(|| panic!("no recorded tool output for {file}"))
        .expected
}

/// Path of a committed MP4 fixture (`testdata/mp4/<name>`).
#[must_use]
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/mp4")
        .join(name)
}

/// Write a short MP4 whose video can be copied and whose only audio is EAC3 5.1, so preparing it
/// needs a stereo downmix: an audio encode and nothing else.
///
/// # Panics
///
/// Panics when ffmpeg is missing or fails; a missing tool must fail the test, not skip it.
///
/// Every ffmpeg a test starts gets a null stdin: with the terminal as stdin, ffmpeg switches it
/// to raw mode, and overlapping runs from parallel tests can leave it that way after the tests.
pub fn write_surround_only_mp4(path: &Path) {
    let status = Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=24:d=0.5"])
        .args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=5.1"])
        .args(["-t", "0.5", "-c:v", "libx264", "-profile:v", "high"])
        .args(["-pix_fmt", "yuv420p", "-c:a", "eac3", "-b:a", "192k"])
        .args(["-metadata:s:a:0", "language=eng", "-movflags", "+faststart"])
        .arg(path)
        .status()
        .expect("ffmpeg must be on PATH to generate test media");
    assert!(
        status.success(),
        "ffmpeg could not generate {}",
        path.display()
    );
}

/// Remux `input` into `output` without encoding: the same streams in another container.
///
/// # Panics
///
/// Panics when ffmpeg is missing or fails.
pub fn remux(input: &Path, output: &Path) {
    let status = Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-map", "0", "-c", "copy"])
        .arg(output)
        .status()
        .expect("ffmpeg must be on PATH to generate test media");
    assert!(
        status.success(),
        "ffmpeg could not remux {}",
        input.display()
    );
}

/// Bytes from a hex string.
///
/// # Panics
///
/// Panics on non-hex input; this is test tooling.
#[must_use]
pub fn hex_decode(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// Lower-case hex of `bytes`.
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// Lower-case hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};

    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Write `contents` to `path` as an executable (0755) script that a test may run right away.
///
/// The file is written by a short-lived `/bin/sh` child, never through a descriptor of the test
/// process. Writing it in-process races on Linux: the test harness runs tests on many threads, and
/// another thread that forks in the moment the write descriptor is open hands a copy of it to its
/// child, which holds it until its `exec` (`O_CLOEXEC` closes it only then). Executing the script
/// while any process holds it open for writing fails with `ETXTBSY` ("Text file busy"). The child
/// here closes the file before it exits, and the test process never owns a write descriptor.
///
/// # Panics
///
/// Panics when the script cannot be written; this is test tooling.
#[cfg(unix)]
pub fn write_script(path: &Path, contents: impl AsRef<[u8]>) {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "write_script"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin
        .write_all(contents.as_ref())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "{}: writing the script failed: {status}",
        path.display()
    );
}

/// Read `testdata/golden/<suite>.json`.
///
/// # Panics
///
/// Panics with the file name when the file is missing or malformed; this is test tooling.
#[must_use]
pub fn load_golden<I: DeserializeOwned, E: DeserializeOwned>(suite: &str) -> Vec<GoldenCase<I, E>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/golden")
        .join(format!("{suite}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let file: GoldenFile<I, E> =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    file.cases
}

#[cfg(test)]
mod recorded_tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Fixture {
        file: String,
    }

    #[derive(Deserialize)]
    struct Anything {}

    #[test]
    fn recorded_tools_cover_every_fixture() {
        let fixtures = load_golden::<Fixture, Anything>("mp4_fixtures");
        for case in &fixtures {
            let recorded = load_recorded(&case.input.file);
            if [
                "plain_faststart.mp4",
                "stereo_surround_subs.mp4",
                "dby1.mp4",
            ]
            .contains(&case.input.file.as_str())
            {
                assert!(
                    recorded
                        .mp4info
                        .as_deref()
                        .is_some_and(|t| t.contains("Track")),
                    "{}",
                    case.input.file
                );
                assert!(
                    recorded
                        .mp4dump
                        .as_deref()
                        .is_some_and(|t| t.contains("[ftyp]")),
                    "{}",
                    case.input.file
                );
                assert!(recorded.ffprobe.is_some(), "{}", case.input.file);
            }
        }
        assert!(
            fixtures
                .iter()
                .any(|c| c.input.file == "syn_fragmented.mp4")
        );
    }
}

#[cfg(test)]
mod fixture_tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        file: String,
    }

    #[derive(Deserialize)]
    struct Expected {
        size: usize,
        sha256: String,
    }

    #[test]
    fn fixtures_match_the_recorded_hashes() {
        let cases = load_golden::<Input, Expected>("mp4_fixtures");
        assert!(cases.len() >= 20);
        for case in cases {
            let bytes = std::fs::read(fixture_path(&case.input.file)).unwrap();
            assert_eq!(bytes.len(), case.expected.size, "{}", case.name);
            assert_eq!(sha256_hex(&bytes), case.expected.sha256, "{}", case.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_cases_in_file_order() {
        let cases = load_golden::<i64, i64>("_selftest");
        let names: Vec<&str> = cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(cases[1].input + 1, cases[1].expected);
    }

    #[cfg(unix)]
    #[test]
    fn a_written_script_is_executable_and_runs_its_contents() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("atvprep-testkit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("tool");
        write_script(&script, "#!/bin/sh\nprintf '%s' \"$1\"\n");
        let mode = fs::metadata(&script).unwrap().permissions().mode();
        let output = Command::new(&script).arg("ran").output().unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(output.stdout, b"ran");
    }
}

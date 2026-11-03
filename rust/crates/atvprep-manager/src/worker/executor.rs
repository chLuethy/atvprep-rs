//! Media-executor primitives that do not start a subprocess.

use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::{Error, Result};

/// Validate a configured media launcher without resolving symlinks.
///
/// # Errors
///
/// The path is unsafe, missing, not regular, writable by group/world, or not executable.
pub fn validate_executable(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::Invalid(
            "executable must be an absolute path".to_owned(),
        ));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| Error::Invalid("executable not found".to_owned()))?;
    if metadata.file_type().is_symlink() {
        return Err(Error::Invalid(
            "executable must not be a symlink".to_owned(),
        ));
    }
    if !metadata.is_file() {
        return Err(Error::Invalid(
            "executable is not a regular file".to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode();
        if mode & 0o002 != 0 {
            return Err(Error::Invalid("executable is world-writable".to_owned()));
        }
        if mode & 0o020 != 0 {
            return Err(Error::Invalid("executable is group-writable".to_owned()));
        }
        if mode & 0o111 == 0 {
            return Err(Error::Invalid("executable is not executable".to_owned()));
        }
    }
    Ok(())
}

/// Refuse shell syntax and manager-controlled command-line options.
///
/// # Errors
///
/// An argument is unsafe, names a path, starts a Python module invocation, or overrides manager policy.
pub fn validate_arguments(arguments: &[String]) -> Result<()> {
    const RESERVED: [&str; 9] = [
        "--delete-original",
        "--output",
        "--quarantine",
        "--manager-parent-fd",
        "--manager-leaf",
        "--manager-device",
        "--manager-inode",
        "--manager-ctime-ns",
        "--prefer-hevc10",
    ];
    for argument in arguments {
        if argument.is_empty()
            || argument.len() > 256
            || !argument
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_./@:+-=".contains(&byte))
        {
            return Err(Error::Invalid(format!(
                "argument rejected (unsafe characters): {argument:?}"
            )));
        }
        if matches!(argument.as_str(), "python3" | "python" | "-m") {
            return Err(Error::Invalid(format!(
                "python module invocation is not allowed: {argument}"
            )));
        }
        if argument == "--" {
            return Err(Error::Invalid(
                "argument rejected (ends the options of atvprep prepare): '--'".to_owned(),
            ));
        }
        if argument.starts_with('/') && !argument.starts_with("/dev/fd/") {
            return Err(Error::Invalid(format!(
                "absolute path argument rejected: {argument:?}"
            )));
        }
        if argument.contains('/') && !argument.starts_with("/dev/fd/") {
            return Err(Error::Invalid(format!(
                "path traversal argument rejected: {argument:?}"
            )));
        }
        let name = argument.split('=').next().unwrap_or(argument);
        if RESERVED.iter().any(|reserved| reserved.starts_with(name))
            || name.starts_with("--manager-")
        {
            return Err(Error::Invalid(format!(
                "argument rejected (reserved for the manager): {argument:?}"
            )));
        }
    }
    Ok(())
}

/// Return only locale, temporary-directory and explicitly configured PATH values for children.
#[must_use]
pub fn sanitized_environment(
    source: &BTreeMap<String, String>,
    path: Option<&str>,
) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for key in ["LC_ALL", "LANG", "LC_CTYPE", "TMPDIR", "TEMP", "TMP"] {
        if let Some(value) = source.get(key) {
            result.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(path) = path {
        result.insert("PATH".to_owned(), path.to_owned());
    }
    result
}

/// Parses atvprep and ffmpeg progress output incrementally.
#[derive(Debug, Default)]
pub struct FfmpegProgress {
    tail: Vec<u8>,
    duration_seconds: Option<f64>,
    announced: bool,
    reported: u8,
    pub stage: Option<String>,
}

impl FfmpegProgress {
    const KEEP: usize = 96;

    #[must_use]
    pub const fn new() -> Self {
        Self {
            tail: Vec::new(),
            duration_seconds: None,
            announced: false,
            reported: 0,
            stage: None,
        }
    }

    /// Feed an arbitrary output chunk and return only newly increased progress.
    ///
    /// # Panics
    /// Only if a built-in parser pattern is changed to an invalid regular expression.
    pub fn feed(&mut self, chunk: &[u8]) -> Option<u8> {
        static STAGE: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
        static ANNOUNCED: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
        static DURATION: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
        static ENCODED: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
        let mut data = self.tail.clone();
        data.extend_from_slice(chunk);
        self.tail = data[data.len().saturating_sub(Self::KEEP)..].to_vec();
        let stage = STAGE.get_or_init(|| {
            regex::bytes::Regex::new(r"ATVPREP-STAGE ([a-z]+)\r?\n").expect("static stage pattern")
        });
        let announced = ANNOUNCED.get_or_init(|| {
            regex::bytes::Regex::new(r"ATVPREP-PROGRESS ([0-9]{1,3})\r?\n")
                .expect("static progress pattern")
        });
        let duration = DURATION.get_or_init(|| {
            regex::bytes::Regex::new(r"Duration:\s*([0-9]+):([0-9]{2}):([0-9]{2}(?:\.[0-9]+)?)")
                .expect("static duration pattern")
        });
        let encoded = ENCODED.get_or_init(|| {
            regex::bytes::Regex::new(r"time=([0-9]+):([0-9]{2}):([0-9]{2}(?:\.[0-9]+)?)")
                .expect("static encoded-time pattern")
        });
        if let Some(captures) = stage.captures_iter(&data).last() {
            self.stage = std::str::from_utf8(&captures[1]).ok().map(str::to_owned);
        }
        if let Some(percent) = announced
            .captures_iter(&data)
            .last()
            .and_then(|captures| std::str::from_utf8(&captures[1]).ok()?.parse::<u16>().ok())
        {
            self.announced = true;
            return self.grow(percent);
        }
        if self.announced {
            return None;
        }
        if self.duration_seconds.is_none() {
            self.duration_seconds = duration
                .captures(&data)
                .and_then(|captures| seconds(&captures))
                .filter(|value| *value > 0.0);
        }
        let duration = self.duration_seconds?;
        let encoded = encoded
            .captures_iter(&data)
            .last()
            .and_then(|captures| seconds(&captures))?;
        let percent = (100.0 * encoded / duration).floor().clamp(0.0, 100.0);
        self.grow(percent.to_string().parse().ok()?)
    }

    fn grow(&mut self, percent: u16) -> Option<u8> {
        let value = u8::try_from(percent.min(99)).ok()?;
        if value <= self.reported {
            None
        } else {
            self.reported = value;
            Some(value)
        }
    }
}
fn seconds(captures: &regex::bytes::Captures<'_>) -> Option<f64> {
    let number = |index| {
        std::str::from_utf8(captures.get(index)?.as_bytes())
            .ok()?
            .parse::<f64>()
            .ok()
    };
    let value = number(1)? * 3600.0 + number(2)? * 60.0 + number(3)?;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsafe_arguments_and_environment_are_refused() {
        for argument in ["--delete-original", "../media", "/tmp/a", "python3", "--"] {
            assert!(
                validate_arguments(&[argument.to_owned()]).is_err(),
                "{argument}"
            );
        }
        validate_arguments(&["--quiet".to_owned(), "/dev/fd/7".to_owned()]).unwrap();
        let source = BTreeMap::from([
            ("TOKEN".to_owned(), "secret".to_owned()),
            ("LANG".to_owned(), "C".to_owned()),
        ]);
        assert_eq!(
            sanitized_environment(&source, Some("/tools"))
                .into_iter()
                .collect::<Vec<_>>(),
            vec![
                ("LANG".to_owned(), "C".to_owned()),
                ("PATH".to_owned(), "/tools".to_owned())
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_must_be_regular_private_and_executable() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("tool");
        fs::write(&binary, b"x").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        validate_executable(&binary).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o722)).unwrap();
        assert!(validate_executable(&binary).is_err());
    }
    #[test]
    fn announced_progress_wins_and_grows() {
        let mut progress = FfmpegProgress::new();
        assert_eq!(
            progress.feed(b"Duration: 00:10:00.00\ntime=00:05:00\n"),
            Some(50)
        );
        assert_eq!(
            progress.feed(b"ATVPREP-STAGE encode\nATVPREP-PROGRESS 51\n"),
            Some(51)
        );
        assert_eq!(progress.feed(b"ATVPREP-PROGRESS 51\n"), None);
        assert_eq!(progress.feed(b"ATVPREP-PROGRESS 100\n"), Some(99));
        assert_eq!(progress.stage.as_deref(), Some("encode"));
    }
    #[test]
    fn split_tokens_are_retained() {
        let mut progress = FfmpegProgress::new();
        assert_eq!(progress.feed(b"Duration: 00:01:"), None);
        assert_eq!(progress.feed(b"00.00\ntime=00:00:30\n"), Some(50));
    }
}

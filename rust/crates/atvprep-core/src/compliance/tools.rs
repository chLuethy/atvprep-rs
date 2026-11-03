//! The external programs a compliance audit reads (`mp4info`, `mp4dump`, `ffprobe`) and the cache
//! the rules of one audit share (replaces the `ContextVar` scope of the Python version).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::ffprobe::Ffprobe;

/// Why a tool could not give its output (the text a rule shows as `rule crashed: …`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError(pub String);

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ToolError {}

/// What an audit reads from the outside world. Real programs in production, recordings in tests.
pub trait Tools {
    /// Standard output of a Bento4 tool (`mp4info`, `mp4dump`) run on `path`.
    ///
    /// # Errors
    ///
    /// [`ToolError`] when the tool is missing or printed nothing.
    fn run_tool(&self, name: &str, path: &Path) -> Result<String, ToolError>;

    /// The ffprobe JSON of `path`.
    ///
    /// # Errors
    ///
    /// [`ToolError`] when ffprobe fails.
    fn ffprobe(&self, path: &Path) -> Result<Value, ToolError>;

    /// Stream indices of audio tracks that duplicate an earlier one; a failure means none.
    fn duplicate_audio(&self, path: &Path, probe: &Value) -> BTreeSet<i64>;
}

/// The real programs, found on `PATH` (or in an explicit list of directories).
#[derive(Debug, Clone)]
pub struct SystemTools {
    ffprobe: Ffprobe,
    search_path: Option<Vec<PathBuf>>,
}

impl Default for SystemTools {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTools {
    #[must_use]
    pub fn new() -> Self {
        Self {
            ffprobe: Ffprobe::new(),
            search_path: None,
        }
    }

    /// Look for the Bento4 tools in these directories instead of `PATH`.
    #[must_use]
    pub fn with_search_path(mut self, dirs: Vec<PathBuf>) -> Self {
        self.search_path = Some(dirs);
        self
    }

    /// Use another `ffprobe` (a fake in tests).
    #[must_use]
    pub fn with_ffprobe(mut self, ffprobe: Ffprobe) -> Self {
        self.ffprobe = ffprobe;
        self
    }

    fn find(&self, name: &str) -> Option<PathBuf> {
        let dirs: Vec<PathBuf> = self.search_path.clone().unwrap_or_else(|| {
            std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default()
        });
        dirs.into_iter()
            .map(|dir| dir.join(name))
            .find(|candidate| {
                std::fs::metadata(candidate)
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    }
}

impl Tools for SystemTools {
    fn run_tool(&self, name: &str, path: &Path) -> Result<String, ToolError> {
        let Some(program) = self.find(name) else {
            return Err(ToolError(format!(
                "'{name}' is required for compliance checks; install via Bento4 \
                 (`brew install bento4` or https://www.bok.net/Bento4/binaries/)"
            )));
        };
        // Some Bento4 tools exit non-zero while still printing useful output: any exit code is
        // fine as long as there is output.
        let output = Command::new(program)
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| ToolError(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if stdout.is_empty() {
            return Err(ToolError(format!(
                "{name} produced no output for {} (exit={}): {}",
                path.display(),
                output.status.code().map_or(-1, i64::from),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(stdout)
    }

    fn ffprobe(&self, path: &Path) -> Result<Value, ToolError> {
        self.ffprobe
            .probe_json(path)
            .map_err(|e| ToolError(e.to_string()))
    }

    fn duplicate_audio(&self, path: &Path, probe: &Value) -> BTreeSet<i64> {
        self.ffprobe.duplicate_audio_indices(path, probe)
    }
}

/// `(path, mtime in ns, size)`: a read is reused only while the file is unchanged.
type StatKey = (PathBuf, i128, u64);

fn stat_key(path: &Path) -> Option<StatKey> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
    Some((path.to_path_buf(), mtime, meta.size()))
}

/// One audit: the rules of one `verify` share the output of each tool for an unchanged file.
///
/// `mp4info`, `mp4dump` and ffprobe each read the whole moov; thirteen rules asking for the same
/// output would start a dozen processes. Entries are keyed by path, mtime and size, a failed call
/// is never cached, and nothing is shared between two `Audit`s.
pub struct Audit<'a> {
    tools: &'a dyn Tools,
    texts: RefCell<HashMap<(String, StatKey), String>>,
    probes: RefCell<HashMap<StatKey, Value>>,
    duplicates: RefCell<HashMap<StatKey, BTreeSet<i64>>>,
}

impl<'a> Audit<'a> {
    #[must_use]
    pub fn new(tools: &'a dyn Tools) -> Self {
        Self {
            tools,
            texts: RefCell::default(),
            probes: RefCell::default(),
            duplicates: RefCell::default(),
        }
    }

    /// Output of a Bento4 tool, read once per file state.
    ///
    /// # Errors
    ///
    /// The tool's [`ToolError`].
    pub fn tool_output(&self, name: &str, path: &Path) -> Result<String, ToolError> {
        let Some(key) = stat_key(path) else {
            return self.tools.run_tool(name, path);
        };
        let key = (name.to_owned(), key);
        if let Some(text) = self.texts.borrow().get(&key) {
            return Ok(text.clone());
        }
        let text = self.tools.run_tool(name, path)?;
        self.texts.borrow_mut().insert(key, text.clone());
        Ok(text)
    }

    /// The ffprobe JSON, read once per file state.
    ///
    /// # Errors
    ///
    /// The tool's [`ToolError`].
    pub fn probe(&self, path: &Path) -> Result<Value, ToolError> {
        let Some(key) = stat_key(path) else {
            return self.tools.ffprobe(path);
        };
        if let Some(probe) = self.probes.borrow().get(&key) {
            return Ok(probe.clone());
        }
        let probe = self.tools.ffprobe(path)?;
        self.probes.borrow_mut().insert(key, probe.clone());
        Ok(probe)
    }

    /// Duplicate audio stream indices (they may need a read of the whole file), once per file state.
    pub fn duplicate_audio(&self, path: &Path, probe: &Value) -> BTreeSet<i64> {
        let Some(key) = stat_key(path) else {
            return self.tools.duplicate_audio(path, probe);
        };
        if let Some(found) = self.duplicates.borrow().get(&key) {
            return found.clone();
        }
        let found = self.tools.duplicate_audio(path, probe);
        self.duplicates.borrow_mut().insert(key, found.clone());
        found
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;

    /// Counts what the audit asks for and answers from fixed data.
    #[derive(Default)]
    struct Counting {
        calls: RefCell<Vec<String>>,
        fail: bool,
    }

    impl Tools for Counting {
        fn run_tool(&self, name: &str, _path: &Path) -> Result<String, ToolError> {
            self.calls.borrow_mut().push(name.to_owned());
            if self.fail {
                Err(ToolError("boom".to_owned()))
            } else {
                Ok(format!("{name} output"))
            }
        }

        fn ffprobe(&self, _path: &Path) -> Result<Value, ToolError> {
            self.calls.borrow_mut().push("ffprobe".to_owned());
            if self.fail {
                Err(ToolError("boom".to_owned()))
            } else {
                Ok(json!({"streams": []}))
            }
        }

        fn duplicate_audio(&self, _path: &Path, _probe: &Value) -> BTreeSet<i64> {
            self.calls.borrow_mut().push("duplicates".to_owned());
            BTreeSet::from([2])
        }
    }

    fn file(content: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("film.mp4");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn count(tools: &Counting, name: &str) -> usize {
        tools.calls.borrow().iter().filter(|c| *c == name).count()
    }

    #[test]
    fn each_tool_runs_once_per_audit() {
        let (_dir, path) = file(b"abc");
        let tools = Counting::default();
        let audit = Audit::new(&tools);
        for _ in 0..3 {
            assert_eq!(
                audit.tool_output("mp4info", &path).unwrap(),
                "mp4info output"
            );
            audit.tool_output("mp4dump", &path).unwrap();
            let probe = audit.probe(&path).unwrap();
            assert_eq!(audit.duplicate_audio(&path, &probe), BTreeSet::from([2]));
        }
        for name in ["mp4info", "mp4dump", "ffprobe", "duplicates"] {
            assert_eq!(count(&tools, name), 1, "{name}");
        }
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let (_dir, path) = file(b"abc");
        let tools = Counting::default();
        let audit = Audit::new(&tools);
        audit.tool_output("mp4info", &path).unwrap();
        fs::write(&path, b"a different, longer content").unwrap();
        audit.tool_output("mp4info", &path).unwrap();
        assert_eq!(count(&tools, "mp4info"), 2);
    }

    #[test]
    fn two_audits_share_nothing() {
        let (_dir, path) = file(b"abc");
        let tools = Counting::default();
        Audit::new(&tools).tool_output("mp4info", &path).unwrap();
        Audit::new(&tools).tool_output("mp4info", &path).unwrap();
        assert_eq!(count(&tools, "mp4info"), 2);
    }

    #[test]
    fn a_failed_call_is_not_cached_and_a_missing_file_bypasses_the_cache() {
        let (_dir, path) = file(b"abc");
        let failing = Counting {
            fail: true,
            ..Counting::default()
        };
        let audit = Audit::new(&failing);
        assert!(audit.tool_output("mp4info", &path).is_err());
        assert!(audit.tool_output("mp4info", &path).is_err());
        assert_eq!(count(&failing, "mp4info"), 2);

        let tools = Counting::default();
        let audit = Audit::new(&tools);
        let missing = Path::new("/definitely/not/here.mp4");
        audit.tool_output("mp4info", missing).unwrap();
        audit.tool_output("mp4info", missing).unwrap();
        assert_eq!(count(&tools, "mp4info"), 2);
    }

    fn script(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn system(dir: &Path) -> SystemTools {
        SystemTools::new().with_search_path(vec![dir.to_path_buf()])
    }

    #[test]
    fn a_missing_tool_has_the_python_text() {
        let empty = tempfile::tempdir().unwrap();
        let error = system(empty.path())
            .run_tool("mp4info", Path::new("x.mp4"))
            .unwrap_err();
        assert_eq!(
            error.0,
            "'mp4info' is required for compliance checks; install via Bento4 (`brew install bento4` or https://www.bok.net/Bento4/binaries/)"
        );
    }

    #[test]
    fn empty_output_is_an_error_with_exit_code_and_stderr() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), "mp4info", "echo '  bad file  ' >&2; exit 3");
        let error = system(dir.path())
            .run_tool("mp4info", Path::new("x.mp4"))
            .unwrap_err();
        assert_eq!(
            error.0,
            "mp4info produced no output for x.mp4 (exit=3): bad file"
        );
    }

    #[test]
    fn output_with_a_non_zero_exit_is_accepted_and_the_path_is_the_only_argument() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), "mp4dump", r#"echo "args:$#:$1"; exit 1"#);
        let text = system(dir.path())
            .run_tool("mp4dump", Path::new("/m/film.mp4"))
            .unwrap();
        assert_eq!(text, "args:1:/m/film.mp4\n");
    }

    #[test]
    fn invalid_utf8_is_replaced_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), "mp4info", r"printf 'ok \377 done'");
        let text = system(dir.path())
            .run_tool("mp4info", Path::new("x.mp4"))
            .unwrap();
        assert!(
            text.starts_with("ok ") && text.ends_with(" done"),
            "{text:?}"
        );
    }

    #[test]
    fn a_file_that_is_not_executable_is_not_a_tool() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("mp4info"), "#!/bin/sh\necho hi\n").unwrap();
        assert!(
            system(dir.path())
                .run_tool("mp4info", Path::new("x.mp4"))
                .is_err()
        );
    }
}

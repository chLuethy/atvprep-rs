//! Opt-in timing of the phases around a fix: ffmpeg, patching, postfix, verify (port of
//! `atvprep/phase_timing.py`).
//!
//! Off unless `ATVPREP_TRACE_PHASES` is set. `1` writes JSON lines to stderr, any other value is a
//! file the lines are appended to. Each line carries the phase name, its wall-clock seconds and,
//! when a path is given, the size of that file when the phase ended.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Map, Value};

pub const ENV_VAR: &str = "ATVPREP_TRACE_PHASES";

/// Where the trace lines go.
pub enum Sink {
    Stderr,
    File(PathBuf),
    Writer(Box<dyn Write + Send>),
}

impl Sink {
    /// The sink a value of [`ENV_VAR`] selects; blank or unset means tracing is off.
    #[must_use]
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.map(str::trim).filter(|v| !v.is_empty())? {
            "1" => Some(Self::Stderr),
            path => Some(Self::File(PathBuf::from(path))),
        }
    }

    fn emit(&mut self, line: &str) {
        // Tracing must never break the work it observes.
        match self {
            Self::Stderr => {
                let _ = writeln!(std::io::stderr(), "{line}");
            }
            Self::File(path) => {
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(file, "{line}");
                }
            }
            Self::Writer(writer) => {
                let _ = writeln!(writer, "{line}");
                let _ = writer.flush();
            }
        }
    }
}

/// Times the enclosed scope as one phase and writes its record when dropped, panics included.
pub struct PhaseGuard {
    sink: Option<Sink>,
    name: String,
    path: Option<PathBuf>,
    started: Instant,
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        let seconds = (self.started.elapsed().as_secs_f64() * 10_000.0).round() / 10_000.0;
        let mut record = Map::new();
        record.insert("phase".to_owned(), Value::from(self.name.as_str()));
        record.insert("seconds".to_owned(), Value::from(seconds));
        if let Some(size) = self
            .path
            .as_deref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
        {
            record.insert("bytes".to_owned(), Value::from(size));
        }
        sink.emit(&Value::Object(record).to_string());
    }
}

/// Time the scope that holds the returned guard as `name`; a no-op while tracing is off.
#[must_use]
pub fn phase(name: &str, path: Option<&Path>) -> PhaseGuard {
    phase_with(
        Sink::parse(std::env::var(ENV_VAR).ok().as_deref()),
        name,
        path,
    )
}

/// Like [`phase`] with an explicit sink (`None` = off).
#[must_use]
pub fn phase_with(sink: Option<Sink>, name: &str, path: Option<&Path>) -> PhaseGuard {
    PhaseGuard {
        sink,
        name: name.to_owned(),
        path: path.map(Path::to_path_buf),
        started: Instant::now(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Mutex};

    use serde_json::Value;

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

    fn records(text: &str) -> Vec<Value> {
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn the_variable_picks_the_sink() {
        assert!(Sink::parse(None).is_none());
        assert!(Sink::parse(Some("  ")).is_none());
        assert!(matches!(Sink::parse(Some("1")), Some(Sink::Stderr)));
        assert!(
            matches!(Sink::parse(Some(" /tmp/t.jsonl ")), Some(Sink::File(p)) if p == Path::new("/tmp/t.jsonl"))
        );
    }

    #[test]
    fn a_writer_gets_one_json_line_with_the_phase_and_seconds() {
        let out = Capture::default();
        drop(phase_with(
            Some(Sink::Writer(Box::new(out.clone()))),
            "x",
            None,
        ));
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let found = records(&text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["phase"], "x");
        assert!(found[0]["seconds"].as_f64().unwrap() >= 0.0);
        assert!(found[0].get("bytes").is_none());
    }

    #[test]
    fn a_file_sink_appends_and_records_the_size_of_the_given_path() {
        let dir = tempfile::tempdir().unwrap();
        let sink = dir.path().join("trace.jsonl");
        let media = dir.path().join("m.bin");
        std::fs::write(&media, [0_u8; 10]).unwrap();

        drop(phase_with(
            Some(Sink::File(sink.clone())),
            "first",
            Some(&media),
        ));
        drop(phase_with(Some(Sink::File(sink.clone())), "second", None));

        let found = records(&std::fs::read_to_string(&sink).unwrap());
        assert_eq!(found[0]["phase"], "first");
        assert_eq!(found[0]["bytes"], 10);
        assert_eq!(found[1]["phase"], "second");
        assert!(found[1].get("bytes").is_none());
    }

    #[test]
    fn without_a_sink_nothing_happens() {
        drop(phase_with(
            None,
            "anything",
            Some(Path::new("/definitely/missing")),
        ));
    }

    #[test]
    fn a_missing_path_leaves_out_the_size() {
        let out = Capture::default();
        drop(phase_with(
            Some(Sink::Writer(Box::new(out.clone()))),
            "x",
            Some(Path::new("/definitely/missing")),
        ));
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert!(records(&text)[0].get("bytes").is_none());
    }

    #[test]
    fn a_block_that_panics_is_still_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let sink = dir.path().join("t.jsonl");
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = phase_with(Some(Sink::File(sink.clone())), "boom", None);
            panic!("block failed");
        }));
        assert!(result.is_err());
        assert_eq!(
            records(&std::fs::read_to_string(&sink).unwrap())[0]["phase"],
            "boom"
        );
    }
}

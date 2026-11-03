//! The real `mp4info`, `mp4dump` and `ffprobe` against the fixtures: the verdict must be the one
//! the Python version recorded with the same programs.
//!
//! This test needs Bento4 and ffmpeg on `PATH`. It fails loudly when they are missing instead of
//! passing silently: a verify that never ran the real tools proves nothing about them.

use std::path::PathBuf;

use atvprep_core::compliance::SystemTools;
use atvprep_core::verify::verify_file;
use atvprep_testkit::{fixture_path, load_golden};
use serde::Deserialize;
use serde_json::{Value, json};

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[derive(Deserialize)]
struct Input {
    kind: String,
    file: Option<String>,
}

#[test]
fn system_tools_reproduce_the_recorded_verdict() {
    for program in ["mp4info", "mp4dump", "ffprobe"] {
        assert!(
            on_path(program),
            "{program} not found on PATH: install Bento4 and ffmpeg to run the real-tool test"
        );
    }
    let tools = SystemTools::new();
    let mut checked = 0;
    for case in load_golden::<Input, Value>("verify") {
        let Some(file) = case.input.file.as_deref() else {
            continue;
        };
        if case.input.kind != "file" || !case.name.ends_with(" recorded") {
            continue;
        }
        let path: PathBuf = fixture_path(file);
        let report = verify_file(&path, &tools, None, None);
        let actual: Vec<Value> = report
            .violations
            .iter()
            .map(|v| json!([v.rule, v.severity.as_str(), v.message, v.track_id]))
            .collect();
        assert_eq!(Value::Array(actual), case.expected["violations"], "{file}");
        checked += 1;
    }
    assert!(
        checked >= 6,
        "only {checked} recorded verdicts were compared"
    );
}

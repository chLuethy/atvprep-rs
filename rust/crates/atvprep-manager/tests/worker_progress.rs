//! Byte-chunk progress and stage transitions match the recorded Python parser.
use atvprep_manager::worker::executor::FfmpegProgress;
use serde_json::{Value, json};
#[test]
fn incremental_progress_matches_python() {
    for case in atvprep_testkit::load_golden::<Vec<String>, Vec<Value>>("worker_progress") {
        let mut parser = FfmpegProgress::new();
        let observed: Vec<Value> = case
            .input
            .iter()
            .map(|chunk| json!({"percent":parser.feed(chunk.as_bytes()),"stage":parser.stage}))
            .collect();
        assert_eq!(observed, case.expected, "{}", case.name);
    }
}

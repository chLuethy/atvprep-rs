//! The conservative compliance projection of a stored inspection (port of
//! `atvprep_manager/compliance.py`).
//!
//! Only a completed `verify` report can establish clean/warning/blocker. An ffprobe payload is
//! provenance; it can only say "unknown".

use atvprep_core::escalation::video_reencode_reason;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub state: &'static str,
    pub reasons: Vec<String>,
}

const CURRENT_FFPROBE: &str =
    "Current ffprobe inspection is available, but AVPlayer compliance requires a verify report.";
const NO_VALID_REPORT: &str = "Verify inspection has no valid violations report.";

fn unknown(reasons: Vec<String>) -> Evaluation {
    Evaluation {
        state: "unknown",
        reasons,
    }
}

/// Project one inspection into a stable media compliance status. The fingerprint fence stops an
/// older inspection from claiming a result for changed bytes.
#[must_use]
pub fn evaluate_inspection(
    tool_name: &str,
    result: &Value,
    inspection_fingerprint: &str,
    media_fingerprint: &str,
) -> Evaluation {
    if inspection_fingerprint != media_fingerprint {
        return unknown(vec![
            "Latest inspection does not match the current media fingerprint.".to_owned(),
        ]);
    }
    match tool_name {
        "ffprobe" => {
            let mut reasons = vec![CURRENT_FFPROBE.to_owned()];
            if let Some(reason) = video_reencode_reason(result, false, false) {
                reasons.push(format!("[warning] video-reencode: {reason}"));
            }
            unknown(reasons)
        }
        "verify" => verify_verdict(result),
        _ => unknown(vec![
            "Latest inspection is not a supported compliance verify report.".to_owned(),
        ]),
    }
}

fn verify_verdict(result: &Value) -> Evaluation {
    let Some(violations) = result.get("violations").and_then(Value::as_array) else {
        return unknown(vec![NO_VALID_REPORT.to_owned()]);
    };
    let mut found: Vec<(&str, &str, &str)> = Vec::new();
    for violation in violations {
        let field = |name: &str| violation.get(name).and_then(Value::as_str);
        match (field("severity"), field("rule"), field("message")) {
            (Some(severity @ ("blocker" | "warning")), Some(rule), Some(message)) => {
                found.push((severity, rule, message));
            }
            _ => return unknown(vec![NO_VALID_REPORT.to_owned()]),
        }
    }
    if found.is_empty() {
        return Evaluation {
            state: "clean",
            reasons: vec!["No compliance violations reported by verify.".to_owned()],
        };
    }
    found.sort_by_key(|(severity, rule, message)| (*severity != "blocker", *rule, *message));
    let blocker = found.iter().any(|(severity, _, _)| *severity == "blocker");
    Evaluation {
        state: if blocker { "blocker" } else { "warning" },
        reasons: found
            .iter()
            .map(|(severity, rule, message)| format!("[{severity}] {rule}: {message}"))
            .collect(),
    }
}

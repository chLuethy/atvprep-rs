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
    /// The video must be re-encoded to meet the requirements (stored with the result).
    pub needs_reencode: bool,
}

const CURRENT_FFPROBE: &str =
    "Current ffprobe inspection is available, but AVPlayer compliance requires a verify report.";
const NO_VALID_REPORT: &str = "Verify inspection has no valid violations report.";

fn unknown(reasons: Vec<String>) -> Evaluation {
    Evaluation {
        state: "unknown",
        reasons,
        needs_reencode: false,
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
            let reencode = video_reencode_reason(result, false, false);
            if let Some(reason) = &reencode {
                reasons.push(format!("[warning] video-reencode: {reason}"));
            }
            Evaluation {
                needs_reencode: reencode.is_some(),
                ..unknown(reasons)
            }
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
            needs_reencode: false,
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
        needs_reencode: found.iter().any(|(_, rule, _)| *rule == "video-reencode"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn warning(rule: &str) -> Value {
        json!({"rule": rule, "severity": "warning", "message": "m"})
    }

    fn vc1_video() -> Value {
        json!({"streams": [{"codec_type": "video", "codec_name": "vc1",
            "pix_fmt": "yuv420p", "width": 720, "height": 480}]})
    }

    #[test]
    fn a_media_analysis_is_unknown_and_records_the_re_encode_need() {
        let needing = evaluate_inspection("ffprobe", &vc1_video(), "f", "f");
        assert_eq!(needing.state, "unknown");
        assert!(needing.needs_reencode);
        let fine = evaluate_inspection("ffprobe", &json!({}), "f", "f");
        assert_eq!(fine.state, "unknown");
        assert!(!fine.needs_reencode);
    }

    #[test]
    fn a_verify_report_yields_state_and_the_re_encode_need() {
        let report = json!({"violations": [warning("video-reencode")]});
        let evaluation = evaluate_inspection("verify", &report, "f", "f");
        assert_eq!(evaluation.state, "warning");
        assert!(evaluation.needs_reencode);

        let blocked = json!({"violations": [warning("video-reencode"),
            {"rule": "no-fragments", "severity": "blocker", "message": "fragmented"}]});
        let evaluation = evaluate_inspection("verify", &blocked, "f", "f");
        assert_eq!(evaluation.state, "blocker");
        assert!(evaluation.needs_reencode);

        let other = json!({"violations": [warning("no-fragments")]});
        assert!(!evaluate_inspection("verify", &other, "f", "f").needs_reencode);
        let clean = json!({"violations": []});
        assert!(!evaluate_inspection("verify", &clean, "f", "f").needs_reencode);
    }

    #[test]
    fn a_stale_or_unsupported_inspection_does_not_claim_a_re_encode_need() {
        let report = json!({"violations": [warning("video-reencode")]});
        assert!(!evaluate_inspection("verify", &report, "old", "new").needs_reencode);
        assert!(!evaluate_inspection("other", &report, "f", "f").needs_reencode);
    }
}

//! The conservative compliance projection of a stored inspection (port of
//! `atvprep_manager/compliance.py`).
//!
//! Only a completed `verify` report can establish clean/warning/blocker. An ffprobe payload is
//! provenance; it can only say "unknown".

use atvprep_core::encode::{EncodeNeed, need_name};
use atvprep_core::escalation::video_reencode_reason;
use serde_json::Value;

/// What an inspection says about the stored Kodierbedarf and Änderungsart of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedUpdate {
    /// The inspection cannot say anything about the need: what is stored stays.
    Keep,
    /// „unbekannt“: the inspection does not belong to these bytes, or the planner could not say.
    Unknown,
    Set {
        need: &'static str,
        change: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub state: &'static str,
    pub reasons: Vec<String>,
    /// The need the report carries (stored with the result).
    pub need: NeedUpdate,
}

const CURRENT_FFPROBE: &str =
    "Current ffprobe inspection is available, but AVPlayer compliance requires a verify report.";
const NO_VALID_REPORT: &str = "Verify inspection has no valid violations report.";

fn unknown(reasons: Vec<String>, need: NeedUpdate) -> Evaluation {
    Evaluation {
        state: "unknown",
        reasons,
        need,
    }
}

/// The need a verify report carries. A report from before the Kodierbedarf has none, so it changes
/// nothing; only a stored value the planner did not produce would be wrong here.
fn reported_need(result: &Value) -> NeedUpdate {
    let field = |name: &str| result.get(name).and_then(Value::as_str);
    let Some(need) = field("encode_need") else {
        return NeedUpdate::Keep;
    };
    let need = EncodeNeed::parse(need).map(|need| need_name(Some(need)));
    let change = match field("planned_change") {
        Some("none") => Some("none"),
        Some("patch") => Some("patch"),
        Some("remux") => Some("remux"),
        Some("audio") => Some("audio"),
        Some("reencode") => Some("reencode"),
        _ => None,
    };
    match (need, change) {
        (Some(need), Some(change)) => NeedUpdate::Set { need, change },
        _ => NeedUpdate::Unknown,
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
        return unknown(
            vec!["Latest inspection does not match the current media fingerprint.".to_owned()],
            NeedUpdate::Unknown,
        );
    }
    match tool_name {
        "ffprobe" => {
            let mut reasons = vec![CURRENT_FFPROBE.to_owned()];
            if let Some(reason) = video_reencode_reason(result, false, false) {
                reasons.push(format!("[warning] video-reencode: {reason}"));
            }
            // A media analysis is no check against the planner.
            unknown(reasons, NeedUpdate::Keep)
        }
        "verify" => verify_verdict(result),
        _ => unknown(
            vec!["Latest inspection is not a supported compliance verify report.".to_owned()],
            NeedUpdate::Unknown,
        ),
    }
}

fn verify_verdict(result: &Value) -> Evaluation {
    let need = reported_need(result);
    let Some(violations) = result.get("violations").and_then(Value::as_array) else {
        return unknown(vec![NO_VALID_REPORT.to_owned()], need);
    };
    let mut found: Vec<(&str, &str, &str)> = Vec::new();
    for violation in violations {
        let field = |name: &str| violation.get(name).and_then(Value::as_str);
        match (field("severity"), field("rule"), field("message")) {
            (Some(severity @ ("blocker" | "warning")), Some(rule), Some(message)) => {
                found.push((severity, rule, message));
            }
            _ => return unknown(vec![NO_VALID_REPORT.to_owned()], need),
        }
    }
    if found.is_empty() {
        return Evaluation {
            state: "clean",
            reasons: vec!["No compliance violations reported by verify.".to_owned()],
            need,
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
        need,
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
    fn a_media_analysis_is_unknown_and_says_nothing_about_the_need() {
        let needing = evaluate_inspection("ffprobe", &vc1_video(), "f", "f");
        assert_eq!(needing.state, "unknown");
        assert_eq!(needing.need, NeedUpdate::Keep);
        // The hint for the operator stays.
        assert!(needing.reasons.iter().any(|r| r.contains("video-reencode")));
    }

    #[test]
    fn a_verify_report_yields_the_state_and_the_need_it_carries() {
        let report = json!({"violations": [warning("no-fragments")],
            "encode_need": "audio", "planned_change": "audio"});
        let evaluation = evaluate_inspection("verify", &report, "f", "f");
        assert_eq!(evaluation.state, "warning");
        assert_eq!(
            evaluation.need,
            NeedUpdate::Set {
                need: "audio",
                change: "audio"
            }
        );
        let clean = json!({"violations": [], "encode_need": "none", "planned_change": "none"});
        assert_eq!(
            evaluate_inspection("verify", &clean, "f", "f").need,
            NeedUpdate::Set {
                need: "none",
                change: "none"
            }
        );
    }

    #[test]
    fn the_need_comes_from_the_report_and_never_from_a_rule_name() {
        let report = json!({"violations": [warning("video-reencode")]});
        assert_eq!(
            evaluate_inspection("verify", &report, "f", "f").need,
            NeedUpdate::Keep
        );
    }

    #[test]
    fn a_report_the_planner_could_not_finish_says_unknown() {
        let report =
            json!({"violations": [], "encode_need": "unknown", "planned_change": "unknown"});
        assert_eq!(
            evaluate_inspection("verify", &report, "f", "f").need,
            NeedUpdate::Unknown
        );
    }

    #[test]
    fn a_stale_or_unsupported_inspection_makes_the_need_unknown() {
        let report =
            json!({"violations": [], "encode_need": "video", "planned_change": "reencode"});
        assert_eq!(
            evaluate_inspection("verify", &report, "old", "new").need,
            NeedUpdate::Unknown
        );
        assert_eq!(
            evaluate_inspection("other", &report, "f", "f").need,
            NeedUpdate::Unknown
        );
    }
}

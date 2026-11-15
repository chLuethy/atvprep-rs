//! The conservative compliance projection of a stored inspection (port of
//! `atvprep_manager/compliance.py`).
//!
//! Only a completed `verify` report can establish clean/warning/blocker. An ffprobe payload is
//! provenance; it can only say "unknown".

use atvprep_core::encode::{ChangeKind, EncodeNeed, PlanStep};
use atvprep_core::escalation::video_reencode_reason;
use serde_json::Value;

use crate::settings::TargetProfile;

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
    /// What the planner would do, kept beside the findings and not among them.
    pub steps: Vec<PlanStep>,
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
        steps: Vec::new(),
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
    let change = field("planned_change").and_then(ChangeKind::parse);
    match (EncodeNeed::parse(need), change) {
        (Some(need), Some(change)) => NeedUpdate::Set {
            need: need.as_str(),
            change: change.as_str(),
        },
        _ => NeedUpdate::Unknown,
    }
}

/// Project one inspection into a stable media compliance status. The fingerprint fence stops an
/// older inspection from claiming a result for changed bytes. The re-encode hint of a media
/// analysis is judged under `profile`, like the Kodierbedarf.
#[must_use]
pub fn evaluate_inspection(
    tool_name: &str,
    result: &Value,
    inspection_fingerprint: &str,
    media_fingerprint: &str,
    profile: &TargetProfile,
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
            if let Some(reason) =
                video_reencode_reason(result, profile.prefer_hevc10, profile.keep_av1)
            {
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
            steps: Vec::new(),
            need,
        };
    }
    found.sort_by_key(|(severity, rule, message)| (*severity != "blocker", *rule, *message));
    let blocker = found.iter().any(|(severity, _, _)| *severity == "blocker");
    let mut reasons: Vec<String> = found
        .iter()
        .map(|(severity, rule, message)| format!("[{severity}] {rule}: {message}"))
        .collect();
    Evaluation {
        state: if blocker { "blocker" } else { "warning" },
        reasons,
        steps: planned_steps(result),
        need,
    }
}

/// The most planned steps a stored result keeps, and the longest one.
const MAX_PLAN_STEPS: usize = 16;
const MAX_PLAN_STEP_CHARS: usize = 200;

/// The encodings the planner names in the report (`planned_steps`), kept as data after the
/// findings: the preview and the items of a Vorbereiten job show them without guessing codec or
/// rate (ADR 0008).
fn planned_steps(result: &Value) -> Vec<PlanStep> {
    result
        .get("planned_steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(PlanStep::from_stored)
        .take(MAX_PLAN_STEPS)
        .map(|step| match step {
            PlanStep::Text { text } => PlanStep::Text {
                text: text.chars().take(MAX_PLAN_STEP_CHARS).collect(),
            },
            other => other,
        })
        .collect()
}

/// A stored result as its readers see it: the findings, and the planned steps apart from them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResult {
    pub reasons: Vec<String>,
    pub steps: Vec<PlanStep>,
}

impl StoredResult {
    /// Read `compliance_reasons` and `planned_steps` as stored. A check from before steps were
    /// data left the planner's text among the reasons as `[plan] …`; it is read as a text step
    /// after the stored ones, so the history stays readable.
    #[must_use]
    pub fn read(reasons: &str, steps: &str) -> Self {
        let array = |text: &str| -> Vec<Value> {
            serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default()
        };
        let (old_plan, findings): (Vec<Value>, Vec<Value>) = array(reasons)
            .into_iter()
            .filter(Value::is_string)
            .partition(|reason| {
                reason
                    .as_str()
                    .is_some_and(|text| text.starts_with(PlanStep::LEGACY_PREFIX))
            });
        Self {
            reasons: findings
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            steps: array(steps)
                .iter()
                .chain(&old_plan)
                .filter_map(PlanStep::from_stored)
                .collect(),
        }
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
        let needing =
            evaluate_inspection("ffprobe", &vc1_video(), "f", "f", &TargetProfile::default());
        assert_eq!(needing.state, "unknown");
        assert_eq!(needing.need, NeedUpdate::Keep);
        // The hint for the operator stays.
        assert!(needing.reasons.iter().any(|r| r.contains("video-reencode")));
    }

    #[test]
    fn the_reencode_hint_of_a_media_analysis_follows_the_profile() {
        let av1 = json!({"streams": [{"codec_type": "video", "codec_name": "av1",
            "pix_fmt": "yuv420p", "width": 1920, "height": 1080}]});
        let hints = |profile: &TargetProfile| {
            evaluate_inspection("ffprobe", &av1, "f", "f", profile)
                .reasons
                .iter()
                .any(|r| r.contains("video-reencode"))
        };
        assert!(hints(&TargetProfile::default()));
        let keep_av1 = TargetProfile {
            keep_av1: true,
            ..TargetProfile::default()
        };
        assert!(!hints(&keep_av1));
    }

    #[test]
    fn a_verify_report_yields_the_state_and_the_need_it_carries() {
        let report = json!({"violations": [warning("no-fragments")],
            "encode_need": "audio", "planned_change": "audio"});
        let evaluation =
            evaluate_inspection("verify", &report, "f", "f", &TargetProfile::default());
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
            evaluate_inspection("verify", &clean, "f", "f", &TargetProfile::default()).need,
            NeedUpdate::Set {
                need: "none",
                change: "none"
            }
        );
    }

    const RATE_STEP: &str = r#"{"kind": "sample_rate", "stream": 1, "from_codec": "ac3",
        "to_codec": "eac3", "from_hz": 44100, "to_hz": 48000}"#;

    fn rate_step() -> PlanStep {
        PlanStep::SampleRate {
            stream: 1,
            from_codec: "ac3".to_owned(),
            to_codec: "eac3".to_owned(),
            from_hz: 44100,
            to_hz: 48000,
        }
    }

    #[test]
    fn the_planned_steps_of_a_verify_report_are_kept_apart_from_its_findings() {
        let step: Value = serde_json::from_str(RATE_STEP).unwrap();
        let report = json!({"violations": [{"rule": "audio-sample-rate", "severity": "warning",
                "message": "ac-3 track id=2 sample rate 44100Hz (expected 48000)"}],
            "encode_need": "audio", "planned_change": "audio", "planned_steps": [step]});
        let evaluation =
            evaluate_inspection("verify", &report, "f", "f", &TargetProfile::default());
        assert_eq!(
            evaluation.reasons,
            ["[warning] audio-sample-rate: ac-3 track id=2 sample rate 44100Hz (expected 48000)"]
        );
        assert_eq!(evaluation.steps, [rate_step()]);
        let clean = json!({"violations": [], "planned_steps": [step]});
        let clean = evaluate_inspection("verify", &clean, "f", "f", &TargetProfile::default());
        assert_eq!(
            clean.reasons,
            ["No compliance violations reported by verify."]
        );
        assert_eq!(clean.steps, [], "a clean file plans nothing");
    }

    #[test]
    fn a_step_in_the_old_text_form_is_read_as_text() {
        let report = json!({"violations": [warning("fast-start")],
            "planned_steps": ["audio stream #1 ac3 44100Hz -> eac3 48000Hz"]});
        let steps =
            evaluate_inspection("verify", &report, "f", "f", &TargetProfile::default()).steps;
        assert_eq!(
            steps,
            [PlanStep::Text {
                text: "audio stream #1 ac3 44100Hz -> eac3 48000Hz".to_owned()
            }]
        );
    }

    #[test]
    fn the_need_comes_from_the_report_and_never_from_a_rule_name() {
        let report = json!({"violations": [warning("video-reencode")]});
        assert_eq!(
            evaluate_inspection("verify", &report, "f", "f", &TargetProfile::default()).need,
            NeedUpdate::Keep
        );
    }

    #[test]
    fn a_report_the_planner_could_not_finish_says_unknown() {
        let report =
            json!({"violations": [], "encode_need": "unknown", "planned_change": "unknown"});
        assert_eq!(
            evaluate_inspection("verify", &report, "f", "f", &TargetProfile::default()).need,
            NeedUpdate::Unknown
        );
    }

    #[test]
    fn a_stale_or_unsupported_inspection_makes_the_need_unknown() {
        let report =
            json!({"violations": [], "encode_need": "video", "planned_change": "reencode"});
        assert_eq!(
            evaluate_inspection("verify", &report, "old", "new", &TargetProfile::default()).need,
            NeedUpdate::Unknown
        );
        assert_eq!(
            evaluate_inspection("other", &report, "f", "f", &TargetProfile::default()).need,
            NeedUpdate::Unknown
        );
    }
}

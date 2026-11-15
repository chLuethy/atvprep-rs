//! The Vorbereiten-Snapshot: the settings a prepare job was queued under, as a type instead of
//! loose JSON. Writers build it and serialize it; readers parse it in one place and get either the
//! type or a named [`Refusal`]. The stored JSON form is byte-equal to what the job queue always
//! wrote, so signatures (ADR 0006/0007/0008) stay valid. Not to be confused with
//! [`crate::snapshots`], the Input-Snapshot of a file's state.
use crate::settings::TargetProfile;
use atvprep_core::encode::{EncodeAllowance, PlanStep};
use serde_json::{Value, json};

/// The policy name of every job that may change files. Version 3 carries a per-item Kodierfreigabe
/// (`encode`, ADR 0008); jobs queued under an older version are refused.
pub const PREPARE_POLICY: &str = "encode-v3";

/// What Prepare is expected to do to one file when it is queued: the Änderungsart and the planned
/// steps (the findings it resolves and the encodings the planner names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedChange {
    pub change: String,
    pub steps: Vec<PlanStep>,
}

/// The operator's explicit approval of a manual job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualApproval {
    pub approved_by: i64,
    pub approved_at: Option<i64>,
    /// The signature of the item, once the job has been signed.
    pub approval_mac: Option<String>,
    pub video_encode_confirmed: bool,
}

/// Who stands behind the job: a signed-in operator, or a signed schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Manual(ManualApproval),
    Schedule {
        schedule_id: i64,
        /// The signed parameters as recorded (see [`crate::approval::ScheduleParams::to_json`]).
        params: Value,
    },
}

/// The settings a Vorbereiten job (or one of its items) was queued under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrepareSnapshot {
    pub profile: TargetProfile,
    pub delete_original: bool,
    pub delete_original_confirmed: bool,
    /// The Kodierfreigabe; a job-level snapshot of a schedule has none, every item has one.
    pub encode: Option<EncodeAllowance>,
    /// The decision to rebuild the file. A schedule never carries it.
    pub force: bool,
    /// The change expected when the item was queued.
    pub planned: Option<PlannedChange>,
    pub origin: Origin,
}

/// Why a snapshot cannot authorize a media-changing job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Outdated,
    NoEncode,
    NoProfile,
    ScheduleUnapproved,
    NoApproval,
    NoSession,
}

impl Refusal {
    /// The message shown to the operator.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::Outdated => "This job predates the current policy. Queue the preparation again.",
            Self::NoEncode => {
                "This job carries no encoding allowance. Queue the preparation again."
            }
            Self::NoProfile => "This job carries no target profile. Queue the preparation again.",
            Self::ScheduleUnapproved => {
                "The schedule behind this job carries no approval. Turn it on again."
            }
            Self::NoApproval => "No explicit approval is recorded for this job.",
            Self::NoSession => "The approval is not bound to a signed-in session.",
        }
    }
}

impl PrepareSnapshot {
    /// The snapshot as stored in `settings_snapshot_json`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "pipeline": "prepare",
            "policy": PREPARE_POLICY,
            "profile": self.profile.to_json(),
            "delete_original": self.delete_original,
            "delete_original_confirmed": self.delete_original_confirmed,
        });
        let Some(object) = value.as_object_mut() else {
            return value;
        };
        if let Some(encode) = self.encode {
            object.insert("encode".to_owned(), json!(encode.as_str()));
        }
        if let Some(planned) = &self.planned {
            object.insert("planned_change".to_owned(), json!(planned.change));
            object.insert("planned_steps".to_owned(), json!(planned.steps));
        }
        match &self.origin {
            Origin::Manual(approval) => {
                object.insert("manual_approval".to_owned(), json!(true));
                object.insert(
                    "video_encode_confirmed".to_owned(),
                    json!(approval.video_encode_confirmed),
                );
                object.insert("force".to_owned(), json!(self.force));
                object.insert("approved_by".to_owned(), json!(approval.approved_by));
                if let Some(at) = approval.approved_at {
                    object.insert("approved_at".to_owned(), json!(at));
                }
                if let Some(mac) = &approval.approval_mac {
                    object.insert("approval_mac".to_owned(), json!(mac));
                }
            }
            Origin::Schedule {
                schedule_id,
                params,
            } => {
                object.insert("manual_approval".to_owned(), json!(false));
                object.insert("schedule_id".to_owned(), json!(schedule_id));
                object.insert("schedule_params".to_owned(), params.clone());
                if self.force {
                    object.insert("force".to_owned(), json!(true));
                }
            }
        }
        value
    }

    /// The Kodierfreigabe an item records, if it is one the policy knows. Reads one key; judges
    /// nothing else (use [`Self::parse`] to authorize).
    #[must_use]
    pub fn recorded_encode(snapshot: &Value) -> Option<EncodeAllowance> {
        snapshot
            .get("encode")
            .and_then(Value::as_str)
            .and_then(EncodeAllowance::parse)
    }

    /// The Zielprofil a snapshot records, if it holds one. Reads one key; judges nothing else.
    #[must_use]
    pub fn recorded_profile(snapshot: &Value) -> Option<TargetProfile> {
        snapshot.get("profile").and_then(TargetProfile::from_json)
    }

    /// Read the stored snapshot of an item, or say why it cannot authorize a media-changing job.
    ///
    /// # Errors
    ///
    /// The named [`Refusal`].
    pub fn parse(snapshot: &Value) -> Result<Self, Refusal> {
        Self::require_current_policy(snapshot)?;
        let encode = Self::recorded_encode(snapshot).ok_or(Refusal::NoEncode)?;
        Self::read(snapshot, encode)
    }

    fn require_current_policy(snapshot: &Value) -> Result<(), Refusal> {
        if snapshot.get("pipeline").and_then(Value::as_str) != Some("prepare")
            || snapshot.get("policy").and_then(Value::as_str) != Some(PREPARE_POLICY)
        {
            return Err(Refusal::Outdated);
        }
        Ok(())
    }

    fn read(snapshot: &Value, encode: EncodeAllowance) -> Result<Self, Refusal> {
        let profile = snapshot
            .get("profile")
            .and_then(TargetProfile::from_json)
            .ok_or(Refusal::NoProfile)?;
        let flag = |key: &str| snapshot.get(key) == Some(&Value::Bool(true));
        let planned = snapshot
            .get("planned_change")
            .and_then(Value::as_str)
            .map(|change| PlannedChange {
                change: change.to_owned(),
                steps: snapshot
                    .get("planned_steps")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(PlanStep::from_stored)
                    .collect(),
            });
        let origin = if let Some(schedule) = snapshot.get("schedule_id").filter(|v| !v.is_null()) {
            // A job from a schedule carries no session approval; the schedule's own signature,
            // checked against its current parameters, stands in for it.
            match (
                schedule.as_i64().filter(|id| *id > 0),
                snapshot.get("schedule_params").filter(|v| v.is_object()),
            ) {
                (Some(schedule_id), Some(params)) => Origin::Schedule {
                    schedule_id,
                    params: params.clone(),
                },
                _ => return Err(Refusal::ScheduleUnapproved),
            }
        } else {
            if !flag("manual_approval") {
                return Err(Refusal::NoApproval);
            }
            let approved_by = snapshot
                .get("approved_by")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or(Refusal::NoSession)?;
            Origin::Manual(ManualApproval {
                approved_by,
                approved_at: snapshot.get("approved_at").and_then(Value::as_i64),
                approval_mac: snapshot
                    .get("approval_mac")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                video_encode_confirmed: flag("video_encode_confirmed"),
            })
        };
        Ok(Self {
            profile,
            delete_original: flag("delete_original"),
            delete_original_confirmed: flag("delete_original_confirmed"),
            encode: Some(encode),
            force: flag("force"),
            planned,
            origin,
        })
    }
}

/// Whether a job-level snapshot belongs to the prepare policy (and so gets item parts).
#[must_use]
pub fn is_prepare(snapshot: &Value) -> bool {
    snapshot.get("policy").and_then(Value::as_str) == Some(PREPARE_POLICY)
}

/// The snapshot of one item: the job's, plus under the prepare policy its own Kodierfreigabe and
/// the change expected when it was queued.
#[must_use]
pub fn with_item(job: &Value, encode: EncodeAllowance, planned: Option<PlannedChange>) -> Value {
    let mut settings = job.clone();
    if is_prepare(job)
        && let Some(object) = settings.as_object_mut()
    {
        object.insert("encode".to_owned(), json!(encode.as_str()));
        if let Some(planned) = planned {
            object.insert("planned_change".to_owned(), json!(planned.change));
            object.insert("planned_steps".to_owned(), json!(planned.steps));
        }
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::TargetProfile;
    use atvprep_core::encode::EncodeAllowance;
    use serde_json::json;

    fn manual() -> PrepareSnapshot {
        PrepareSnapshot {
            profile: TargetProfile::default(),
            delete_original: true,
            delete_original_confirmed: false,
            encode: Some(EncodeAllowance::Audio),
            force: true,
            planned: Some(PlannedChange {
                change: "remux".to_owned(),
                steps: vec![
                    PlanStep::Text {
                        text: "[audio] aac".to_owned(),
                    },
                    PlanStep::SampleRate {
                        stream: 1,
                        from_codec: "ac3".to_owned(),
                        to_codec: "eac3".to_owned(),
                        from_hz: 44100,
                        to_hz: 48000,
                    },
                ],
            }),
            origin: Origin::Manual(ManualApproval {
                approved_by: 7,
                approved_at: Some(1_700_000_000),
                approval_mac: Some("ab".repeat(32)),
                video_encode_confirmed: false,
            }),
        }
    }

    fn schedule() -> PrepareSnapshot {
        PrepareSnapshot {
            profile: TargetProfile::default(),
            delete_original: false,
            delete_original_confirmed: false,
            encode: Some(EncodeAllowance::Copy),
            force: false,
            planned: Some(PlannedChange {
                change: "none".to_owned(),
                steps: Vec::new(),
            }),
            origin: Origin::Schedule {
                schedule_id: 3,
                params: json!({"schedule_id":3,"action":"fix","root_id":null,
                    "compliance_state":"non_compliant","max_files":5,"delete_original":false,
                    "encode":"copy","profile":TargetProfile::default().to_json()}),
            },
        }
    }

    #[test]
    fn serialized_form_is_byte_equal_to_the_recorded_golden_files() {
        // Signatures of approvals and schedules rest on this form (ADR 0006/0007/0008).
        assert_eq!(
            manual().to_json().to_string(),
            include_str!("../../../testdata/golden/prepare_snapshot_manual.json").trim_end()
        );
        assert_eq!(
            schedule().to_json().to_string(),
            include_str!("../../../testdata/golden/prepare_snapshot_schedule.json").trim_end()
        );
    }

    #[test]
    fn each_variant_parses_back_to_what_was_built() {
        assert_eq!(PrepareSnapshot::parse(&manual().to_json()), Ok(manual()));
        assert_eq!(
            PrepareSnapshot::parse(&schedule().to_json()),
            Ok(schedule())
        );
    }

    fn current() -> Value {
        json!({"pipeline":"prepare","policy":PREPARE_POLICY,"profile":TargetProfile::default().to_json(),
            "encode":"copy","manual_approval":true,"approved_by":5})
    }

    fn refusal_of(edit: impl FnOnce(&mut Value)) -> Refusal {
        let mut snapshot = current();
        edit(&mut snapshot);
        PrepareSnapshot::parse(&snapshot).expect_err("must be refused")
    }

    #[test]
    fn a_snapshot_of_an_older_policy_is_refused_with_the_known_message() {
        let refusal = refusal_of(|s| s["policy"] = json!("encode-v2"));
        assert_eq!(refusal, Refusal::Outdated);
        assert_eq!(
            refusal.message(),
            "This job predates the current policy. Queue the preparation again."
        );
        assert_eq!(
            PrepareSnapshot::parse(&json!({"pipeline":"verify"})),
            Err(Refusal::Outdated)
        );
    }

    #[test]
    fn missing_pieces_are_refused_by_name() {
        assert_eq!(
            refusal_of(|s| {
                s.as_object_mut().unwrap().remove("encode");
            }),
            Refusal::NoEncode
        );
        assert_eq!(
            refusal_of(|s| s["encode"] = json!("bogus")),
            Refusal::NoEncode
        );
        assert_eq!(
            refusal_of(|s| {
                s.as_object_mut().unwrap().remove("profile");
            }),
            Refusal::NoProfile
        );
        assert_eq!(
            refusal_of(|s| s["manual_approval"] = json!(false)),
            Refusal::NoApproval
        );
        assert_eq!(
            refusal_of(|s| s["approved_by"] = json!(0)),
            Refusal::NoSession
        );
    }

    #[test]
    fn a_schedule_job_without_complete_schedule_data_is_refused() {
        assert_eq!(
            refusal_of(|s| s["schedule_id"] = json!(0)),
            Refusal::ScheduleUnapproved
        );
        assert_eq!(
            refusal_of(|s| s["schedule_id"] = json!(4)),
            Refusal::ScheduleUnapproved
        );
        let mut snapshot = current();
        snapshot["schedule_id"] = json!(4);
        snapshot["schedule_params"] = json!({});
        assert!(PrepareSnapshot::parse(&snapshot).is_ok());
        // A null schedule id is no schedule: the manual rules apply.
        assert!(
            PrepareSnapshot::parse(&{
                let mut s = current();
                s["schedule_id"] = Value::Null;
                s
            })
            .is_ok()
        );
    }

    #[test]
    fn the_item_part_adds_encode_and_planned_change_to_a_job_snapshot() {
        let job = json!({"pipeline":"prepare","policy":PREPARE_POLICY,"x":1});
        let item = with_item(
            &job,
            EncodeAllowance::Audio,
            Some(PlannedChange {
                change: "remux".to_owned(),
                steps: vec![PlanStep::Text {
                    text: "a".to_owned(),
                }],
            }),
        );
        assert_eq!(item["encode"], "audio");
        assert_eq!(item["planned_change"], "remux");
        assert_eq!(
            item["planned_steps"],
            json!([{"kind": "text", "text": "a"}])
        );
        let other = json!({"policy":"other"});
        assert_eq!(with_item(&other, EncodeAllowance::Copy, None), other);
    }
}

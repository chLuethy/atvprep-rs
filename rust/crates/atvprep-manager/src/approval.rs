//! Cryptographic binding for explicit, copy-only manual approvals.
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::fmt::Write;

type HmacSha256 = Hmac<Sha256>;
const MAX_AGE: i64 = 24 * 3600;

/// The policy name of every job that may change files. Version 2 carries a per-item `reencode`
/// decision (ADR 0007); jobs queued under `copy-only-v1` are refused.
pub const PREPARE_POLICY: &str = "copy-only-v2";

/// Explain why a snapshot cannot authorize a media-changing job.
#[must_use]
pub fn prepare_policy_block(snapshot: &Value) -> Option<&'static str> {
    if snapshot.get("pipeline").and_then(Value::as_str) != Some("prepare")
        || snapshot.get("policy").and_then(Value::as_str) != Some(PREPARE_POLICY)
    {
        return Some("This job predates the copy-only policy. Queue the fix again.");
    }
    if !snapshot.get("reencode").is_some_and(Value::is_boolean) {
        return Some("This job carries no re-encode decision. Queue the fix again.");
    }
    if let Some(schedule) = snapshot.get("schedule_id").filter(|v| !v.is_null()) {
        // A job from a schedule carries no session approval; the schedule's own signature,
        // checked against its current parameters, stands in for it (see `check_schedule`).
        if schedule.as_i64().is_some_and(|id| id > 0)
            && snapshot
                .get("schedule_params")
                .is_some_and(Value::is_object)
        {
            return None;
        }
        return Some("The schedule behind this job carries no approval. Turn it on again.");
    }
    if snapshot.get("manual_approval") != Some(&Value::Bool(true)) {
        return Some("No explicit approval is recorded for this job.");
    }
    if snapshot
        .get("approved_by")
        .and_then(Value::as_i64)
        .is_none_or(|id| id <= 0)
    {
        return Some("The approval is not bound to a signed-in session.");
    }
    None
}

/// The parameters an operator signs when turning on a schedule that changes files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduleParams<'a> {
    pub schedule_id: i64,
    pub action: &'a str,
    pub root_id: Option<i64>,
    pub compliance_state: Option<&'a str>,
    pub max_files: i64,
    /// The `delete_original` setting at the time of signing and of each run.
    pub delete_original: bool,
    /// Whether the schedule may take files that need a re-encode (ADR 0007).
    pub allow_reencode: bool,
}

impl ScheduleParams<'_> {
    /// The parameters as recorded in a job snapshot.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "schedule_id": self.schedule_id,
            "action": self.action,
            "root_id": self.root_id,
            "compliance_state": self.compliance_state,
            "max_files": self.max_files,
            "delete_original": self.delete_original,
            "allow_reencode": self.allow_reencode,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").expect("writing to String");
    }
    result
}

/// Lowercase 64-character hex only; anything else cannot be a signature.
fn unhex(signature: &str) -> Option<Vec<u8>> {
    let bytes = signature.as_bytes();
    if bytes.len() != 64
        || !bytes
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return None;
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|v| u8::from_str_radix(v, 16).ok())
        })
        .collect()
}

/// Exact item identity bound into the approval MAC.
#[derive(Clone, Copy)]
pub struct ApprovalItem<'a> {
    pub job_id: i64,
    pub media_id: i64,
    pub action: &'a str,
    pub identity: &'a [Value],
    /// Whether this item may be re-encoded (ADR 0007). Signed, so it cannot be added later.
    pub reencode: bool,
    /// The `delete_original` setting recorded when the job was queued. Signed as well.
    pub delete_original: bool,
}

/// A verifier derived from an external secret, never from database state.
pub struct ApprovalVerifier {
    key: [u8; 32],
}

impl ApprovalVerifier {
    /// Derive the approval key; empty secrets cannot authorize work.
    #[must_use]
    pub fn new(secret: &[u8]) -> Option<Self> {
        if secret.is_empty() {
            return None;
        }
        let mut mac = HmacSha256::new_from_slice(secret).ok()?;
        mac.update(b"atvprep-approval-v2");
        Some(Self {
            key: mac.finalize().into_bytes().into(),
        })
    }

    fn mac(&self, item: &ApprovalItem<'_>, approved_by: i64, approved_at: i64) -> HmacSha256 {
        let mut parts = vec![
            json!("approval-v2"),
            json!(item.job_id),
            json!(item.media_id),
            json!(item.action),
            json!(approved_by),
            json!(approved_at),
            json!(item.identity),
        ];
        // The decisions are appended only when one is set, so an item that copies and keeps its
        // original signs exactly as before; setting either later changes the array and the MAC.
        if item.reencode || item.delete_original {
            parts.push(json!({"reencode": item.reencode, "delete_original": item.delete_original}));
        }
        self.mac_over(&Value::Array(parts))
    }

    /// Sign a schedule's parameters; the operator's approval to let it change files.
    #[must_use]
    pub fn sign_schedule(
        &self,
        params: &ScheduleParams<'_>,
        approved_by: i64,
        approved_at: i64,
    ) -> String {
        hex(&self
            .schedule_mac(params, approved_by, approved_at)
            .finalize()
            .into_bytes())
    }

    fn schedule_mac(
        &self,
        params: &ScheduleParams<'_>,
        approved_by: i64,
        approved_at: i64,
    ) -> HmacSha256 {
        self.mac_over(&json!([
            "schedule-approval-v2",
            params.schedule_id,
            params.action,
            params.root_id,
            params.compliance_state,
            params.max_files,
            params.delete_original,
            params.allow_reencode,
            approved_by,
            approved_at
        ]))
    }

    /// Check a stored schedule approval against the schedule's *current* parameters.
    /// A schedule approval does not age: it ends when the parameters change or the schedule
    /// is paused, which the caller enforces by clearing it.
    #[must_use]
    pub fn check_schedule(
        &self,
        params: &ScheduleParams<'_>,
        approved_by: Option<i64>,
        approved_at: Option<i64>,
        signature: Option<&str>,
    ) -> Option<&'static str> {
        let (Some(approved_by), Some(approved_at), Some(signature)) =
            (approved_by, approved_at, signature)
        else {
            return Some("The schedule is not approved to change files.");
        };
        match unhex(signature) {
            Some(bytes)
                if self
                    .schedule_mac(params, approved_by, approved_at)
                    .verify_slice(&bytes)
                    .is_ok() =>
            {
                None
            }
            _ => Some("The schedule approval does not match its current settings."),
        }
    }

    fn mac_over(&self, data: &Value) -> HmacSha256 {
        let encoded = serde_json::to_string(&data).expect("JSON values serialize");
        let mut ascii = String::new();
        for ch in encoded.chars() {
            if ch.is_ascii() && ch != '\u{7f}' {
                ascii.push(ch);
            } else {
                let mut buf = [0u16; 2];
                for unit in ch.encode_utf16(&mut buf) {
                    write!(ascii, "\\u{unit:04x}").expect("writing to String");
                }
            }
        }
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("HMAC accepts any key");
        mac.update(ascii.as_bytes());
        mac
    }

    /// Sign the job, media, action, approver, timestamp and file identity.
    #[must_use]
    pub fn sign(&self, item: &ApprovalItem<'_>, approved_by: i64, approved_at: i64) -> String {
        hex(&self
            .mac(item, approved_by, approved_at)
            .finalize()
            .into_bytes())
    }

    /// Check the signature and age using a caller-provided current time.
    #[must_use]
    pub fn check(
        &self,
        snapshot: &Value,
        item: &ApprovalItem<'_>,
        now: i64,
    ) -> Option<&'static str> {
        let Some(approved_at) = snapshot.get("approved_at").and_then(Value::as_i64) else {
            return Some("The approval is not signed. Queue the fix again.");
        };
        let Some(signature) = snapshot.get("approval_mac").and_then(Value::as_str) else {
            return Some("The approval is not signed. Queue the fix again.");
        };
        let Some(approved_by) = snapshot.get("approved_by").and_then(Value::as_i64) else {
            return Some("The approval is not bound to a signed-in session.");
        };
        let decoded = unhex(signature);
        if decoded.as_deref().is_none_or(|value| {
            self.mac(item, approved_by, approved_at)
                .verify_slice(value)
                .is_err()
        }) {
            return Some("The approval does not belong to this job. Queue the fix again.");
        }
        let age = now.saturating_sub(approved_at);
        if age > MAX_AGE {
            return Some("The approval has expired. Queue the fix again.");
        }
        if age < -300 {
            return Some("The approval is dated in the future.");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalItem, ApprovalVerifier, ScheduleParams, prepare_policy_block};
    use serde_json::{Value, json};

    const PARAMS: ScheduleParams<'static> = ScheduleParams {
        schedule_id: 3,
        action: "fix",
        root_id: Some(1),
        compliance_state: Some("warning"),
        max_files: 25,
        delete_original: false,
        allow_reencode: false,
    };

    #[test]
    fn a_schedule_signature_binds_every_parameter() {
        let verifier = ApprovalVerifier::new(b"secret").unwrap();
        let mac = verifier.sign_schedule(&PARAMS, 5, 1000);
        assert_eq!(
            verifier.check_schedule(&PARAMS, Some(5), Some(1000), Some(&mac)),
            None
        );
        let other = |p: ScheduleParams<'static>| {
            verifier.check_schedule(&p, Some(5), Some(1000), Some(&mac))
        };
        assert!(
            other(ScheduleParams {
                schedule_id: 4,
                ..PARAMS
            })
            .is_some()
        );
        assert!(
            other(ScheduleParams {
                action: "convert",
                ..PARAMS
            })
            .is_some()
        );
        assert!(
            other(ScheduleParams {
                root_id: None,
                ..PARAMS
            })
            .is_some()
        );
        assert!(
            other(ScheduleParams {
                compliance_state: None,
                ..PARAMS
            })
            .is_some()
        );
        assert!(
            other(ScheduleParams {
                max_files: 26,
                ..PARAMS
            })
            .is_some()
        );
        assert!(
            other(ScheduleParams {
                delete_original: true,
                ..PARAMS
            })
            .is_some()
        );
        assert!(
            other(ScheduleParams {
                allow_reencode: true,
                ..PARAMS
            })
            .is_some(),
            "the re-encode permission is signed"
        );
        assert!(
            verifier
                .check_schedule(&PARAMS, Some(6), Some(1000), Some(&mac))
                .is_some()
        );
        assert!(
            verifier
                .check_schedule(&PARAMS, Some(5), Some(1001), Some(&mac))
                .is_some()
        );
        assert!(verifier.check_schedule(&PARAMS, None, None, None).is_some());
        // A job-item signature never doubles as a schedule signature.
        let item = ApprovalItem {
            job_id: 3,
            media_id: 1,
            action: "fix",
            identity: &[],
            reencode: false,
            delete_original: false,
        };
        assert_ne!(verifier.sign(&item, 5, 1000), mac);
        let stranger = ApprovalVerifier::new(b"other").unwrap();
        assert!(
            stranger
                .check_schedule(&PARAMS, Some(5), Some(1000), Some(&mac))
                .is_some()
        );
    }

    #[test]
    fn a_schedule_snapshot_passes_the_policy_only_with_its_parameters() {
        let snapshot = json!({"pipeline":"prepare","policy":"copy-only-v2","reencode":false,"manual_approval":false,
            "schedule_id":3,"schedule_params":{"action":"fix"}});
        assert_eq!(prepare_policy_block(&snapshot), None);
        let mut bare = snapshot.clone();
        bare.as_object_mut().unwrap().remove("schedule_params");
        assert!(prepare_policy_block(&bare).is_some());
        let mut wrong = snapshot;
        wrong["schedule_id"] = json!("3");
        assert!(prepare_policy_block(&wrong).is_some());
    }

    #[test]
    fn signatures_match_python_and_bind_all_identity_fields() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../../../testdata/golden/approval_mac.json"))
                .unwrap();
        let verifier = ApprovalVerifier::new(b"fixture secret").unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let identity = case["input"]["identity"].as_array().unwrap();
            let item = ApprovalItem {
                job_id: 7,
                media_id: 9,
                action: case["input"]["action"].as_str().unwrap(),
                identity,
                reencode: false,
                delete_original: false,
            };
            let signature = verifier.sign(&item, 5, 1000);
            assert_eq!(signature, case["expected"].as_str().unwrap());
            let snapshot = json!({"approved_by":5,"approved_at":1000,"approval_mac":signature});
            assert_eq!(verifier.check(&snapshot, &item, 1000), None);
            assert!(
                verifier
                    .check(
                        &snapshot,
                        &ApprovalItem {
                            media_id: 10,
                            ..item
                        },
                        1000
                    )
                    .is_some()
            );
            assert!(verifier.check(&snapshot, &item, 1000 + 86401).is_some());
            assert!(verifier.check(&snapshot, &item, 699).is_some());
            assert!(verifier.check(&json!({"approved_by":5,"approved_at":1000,"approval_mac":signature.to_uppercase()}),&item,1000).is_some());
        }
    }

    #[test]
    fn policy_requires_an_explicit_bound_manual_approval() {
        let mut snapshot = json!({"pipeline":"prepare","policy":"copy-only-v2","reencode":false,"manual_approval":true,"approved_by":5});
        assert_eq!(prepare_policy_block(&snapshot), None);
        snapshot["schedule_id"] = Value::Null;
        snapshot["approved_by"] = json!(true);
        assert!(prepare_policy_block(&snapshot).is_some());
    }

    #[test]
    fn a_job_signature_binds_the_re_encode_decision_and_delete_original() {
        let verifier = ApprovalVerifier::new(b"secret").unwrap();
        let base = ApprovalItem {
            job_id: 7,
            media_id: 9,
            action: "fix",
            identity: &[],
            reencode: false,
            delete_original: false,
        };
        let signature = verifier.sign(&base, 5, 1000);
        let snapshot = json!({"approved_by":5,"approved_at":1000,"approval_mac":signature});
        assert_eq!(verifier.check(&snapshot, &base, 1000), None);
        for tampered in [
            ApprovalItem {
                reencode: true,
                ..base
            },
            ApprovalItem {
                delete_original: true,
                ..base
            },
        ] {
            assert!(
                verifier.check(&snapshot, &tampered, 1000).is_some(),
                "a decision added after signing must not verify"
            );
        }
        let signed_yes = ApprovalItem {
            reencode: true,
            delete_original: true,
            ..base
        };
        let yes = json!({"approved_by":5,"approved_at":1000,"approval_mac":verifier.sign(&signed_yes,5,1000)});
        assert_eq!(verifier.check(&yes, &signed_yes, 1000), None);
        assert!(
            verifier.check(&yes, &base, 1000).is_some(),
            "a decision removed after signing must not verify"
        );
    }

    #[test]
    fn only_policy_v2_with_a_re_encode_decision_passes() {
        let v2 = json!({"pipeline":"prepare","policy":"copy-only-v2","reencode":false,"manual_approval":true,"approved_by":5});
        assert_eq!(prepare_policy_block(&v2), None);
        let mut v1 = v2.clone();
        v1["policy"] = json!("copy-only-v1");
        assert!(
            prepare_policy_block(&v1)
                .unwrap()
                .contains("Queue the fix again")
        );
        let mut missing = v2.clone();
        missing.as_object_mut().unwrap().remove("reencode");
        assert!(prepare_policy_block(&missing).is_some());
        let mut text = v2;
        text["reencode"] = json!("true");
        assert!(prepare_policy_block(&text).is_some());
    }
}

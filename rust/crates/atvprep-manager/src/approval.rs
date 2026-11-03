//! Cryptographic binding for explicit, copy-only manual approvals.
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::fmt::Write;

type HmacSha256 = Hmac<Sha256>;
const MAX_AGE: i64 = 24 * 3600;

/// Explain why a snapshot cannot authorize a media-changing job.
#[must_use]
pub fn prepare_policy_block(snapshot: &Value) -> Option<&'static str> {
    if snapshot.get("pipeline").and_then(Value::as_str) != Some("prepare")
        || snapshot.get("policy").and_then(Value::as_str) != Some("copy-only-v1")
    {
        return Some("This job predates the copy-only policy. Queue the fix again.");
    }
    if snapshot.get("schedule_id").is_some_and(|v| !v.is_null()) {
        return Some("Scheduled jobs may not change media. Queue the fix by hand.");
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

/// Exact item identity bound into the approval MAC.
#[derive(Clone, Copy)]
pub struct ApprovalItem<'a> {
    pub job_id: i64,
    pub media_id: i64,
    pub action: &'a str,
    pub identity: &'a [Value],
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
        let data = json!([
            "approval-v2",
            item.job_id,
            item.media_id,
            item.action,
            approved_by,
            approved_at,
            item.identity
        ]);
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
        let bytes = self
            .mac(item, approved_by, approved_at)
            .finalize()
            .into_bytes();
        let mut result = String::with_capacity(64);
        for byte in bytes {
            write!(result, "{byte:02x}").expect("writing to String");
        }
        result
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
        let bytes = signature.as_bytes();
        let decoded = if bytes.len() == 64
            && bytes
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        {
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    std::str::from_utf8(pair)
                        .ok()
                        .and_then(|v| u8::from_str_radix(v, 16).ok())
                })
                .collect::<Option<Vec<_>>>()
        } else {
            None
        };
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
    use super::{ApprovalItem, ApprovalVerifier, prepare_policy_block};
    use serde_json::{Value, json};

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
        let mut snapshot = json!({"pipeline":"prepare","policy":"copy-only-v1","manual_approval":true,"approved_by":5});
        assert_eq!(prepare_policy_block(&snapshot), None);
        snapshot["schedule_id"] = json!(1);
        assert!(prepare_policy_block(&snapshot).is_some());
        snapshot["schedule_id"] = Value::Null;
        snapshot["approved_by"] = json!(true);
        assert!(prepare_policy_block(&snapshot).is_some());
    }
}

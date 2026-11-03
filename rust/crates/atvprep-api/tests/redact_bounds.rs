//! Recorded Python diagnostics, byte tails and structured traversal bounds.
use atvprep_api::{bounds, redact};
use serde_json::{Value, json};
#[test]
fn diagnostic_redaction_and_bounds_match_python() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../testdata/golden/redact_bounds.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        let value = &input["value"];
        let observed = match input["kind"].as_str().unwrap() {
            "text" => json!(redact::redact_text(value.as_str().unwrap())),
            "tail" => json!(redact::redact_log_tail(
                value.as_str().unwrap(),
                usize::try_from(input["limit"].as_u64().unwrap()).unwrap()
            )),
            "inspection" => redact::redact_inspection_value(value, 8192),
            "event" => redact::bounded_event_payload(value),
            "needs_bound" => json!(bounds::value_needs_bound(value)),
            _ => panic!("unknown redaction case"),
        };
        assert_eq!(observed, case["expected"], "{}", case["name"]);
    }
}

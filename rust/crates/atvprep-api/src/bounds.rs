//! Hard JSON response bounds used by API views and event payloads.

use serde_json::Value;

pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_ELEMENT_BYTES: usize = 16 * 1024;
pub const MAX_INSPECTION_BYTES: usize = 56 * 1024;

/// Return a JSON value that fits a response element bound, replacing oversize values safely.
#[must_use]
pub fn bounded_value(value: &Value, limit: usize) -> Value {
    if serde_json::to_vec(value).map_or(true, |encoded| encoded.len() > limit) {
        Value::String("[truncated]".to_owned())
    } else {
        value.clone()
    }
}

/// Bound a page to both the per-element and aggregate API response limits.
#[must_use]
pub fn bounded_page(values: &[Value]) -> Vec<Value> {
    let mut result = Vec::new();
    let mut used = 2_usize;
    for value in values {
        let value = bounded_value(value, MAX_ELEMENT_BYTES);
        let size = serde_json::to_vec(&value).map_or(MAX_RESPONSE_BYTES, |encoded| encoded.len());
        if used.saturating_add(size).saturating_add(1) > MAX_RESPONSE_BYTES {
            break;
        }
        used = used.saturating_add(size).saturating_add(1);
        result.push(value);
    }
    result
}

/// Detect oversized tainted values while bounding traversal depth and item count.
#[must_use]
pub fn value_needs_bound(value: &Value) -> bool {
    fn check(value: &Value, depth: usize) -> bool {
        if depth >= 8 {
            return true;
        }
        match value {
            Value::String(text) => text.len() > 2048,
            Value::Array(items) => {
                items.len() > 64 || items.iter().take(64).any(|value| check(value, depth + 1))
            }
            Value::Object(items) => {
                items.len() > 64
                    || items.iter().take(64).any(|(key, value)| {
                        depth + 1 >= 8 || key.len() > 2048 || check(value, depth + 1)
                    })
            }
            _ => false,
        }
    }
    check(value, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_and_elements_are_bounded() {
        let oversized = Value::String("x".repeat(MAX_ELEMENT_BYTES + 1));
        assert_eq!(
            bounded_value(&oversized, MAX_ELEMENT_BYTES),
            Value::String("[truncated]".to_owned())
        );
        assert_eq!(bounded_page(&[oversized]).len(), 1);
    }
}

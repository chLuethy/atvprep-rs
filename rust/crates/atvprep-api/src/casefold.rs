//! Python 3.13-compatible full Unicode case folding for title ordering.
use caseless::Caseless;

/// Fold with Python's Unicode 15.1 table. `caseless` uses Unicode 16; these 27
/// characters were added after Python's table and must remain unchanged.
#[must_use]
pub fn python_casefold(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        if matches!(
            ch,
            '\u{1c89}' | '\u{a7cb}' | '\u{a7cc}' | '\u{a7da}' | '\u{a7dc}' | '\u{10d50}'
                ..='\u{10d65}'
        ) {
            result.push(ch);
        } else {
            result.extend(std::iter::once(ch).default_case_fold());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::python_casefold;
    use sha2::{Digest, Sha256};

    #[test]
    fn matches_python_for_every_unicode_scalar() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../testdata/golden/casefold.json")).unwrap();
        let expected = fixture["cases"][0]["expected"]["sha256"].as_str().unwrap();
        let mut digest = Sha256::new();
        for codepoint in 0..=0x0010_ffff_u32 {
            let Some(ch) = char::from_u32(codepoint) else {
                continue;
            };
            let folded = python_casefold(&ch.to_string());
            digest.update(codepoint.to_be_bytes());
            digest.update([u8::try_from(folded.len()).unwrap()]);
            digest.update(folded.as_bytes());
        }
        assert_eq!(format!("{:x}", digest.finalize()), expected);
    }
}

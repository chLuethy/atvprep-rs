//! Python's way of printing values, for messages that must read the same.

use std::fmt::Write as _;

use serde_json::Value;

/// `repr(str)`: single quotes unless the text holds a single quote and no double quote.
#[must_use]
pub fn repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `str(value)` for the JSON values ffprobe produces: numbers and strings as they are, a missing
/// value or `null` as `None`, booleans as `True`/`False`.
#[must_use]
pub fn str_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::Bool(false)) => "False".to_owned(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn repr_picks_the_quote_like_python() {
        assert_eq!(repr_str("mov,mp4"), "'mov,mp4'");
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("it's \"x\""), "'it\\'s \"x\"'");
        assert_eq!(repr_str("a\\b\n"), "'a\\\\b\\n'");
        assert_eq!(repr_str("\u{1}ä"), "'\\x01ä'");
    }

    #[test]
    fn str_of_matches_python_formatting() {
        assert_eq!(str_of(None), "None");
        assert_eq!(str_of(Some(&json!(null))), "None");
        assert_eq!(str_of(Some(&json!(3))), "3");
        assert_eq!(str_of(Some(&json!("x"))), "x");
        assert_eq!(str_of(Some(&json!(true))), "True");
    }
}

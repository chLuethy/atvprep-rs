//! JSON text the way Python's `json.dumps(value, indent=2)` prints it: two-space indent and every
//! non-ASCII character escaped as `\uXXXX` (surrogate pairs above the BMP).

use std::fmt::Write as _;

use serde::Serialize;

/// `json.dumps(value, indent=2)`; field order is the declaration order of the serialised structs.
///
/// # Panics
///
/// Panics if `value` cannot be serialised to JSON (maps with non-string keys); the values this
/// crate prints never are.
#[must_use]
pub fn dumps_indent2<T: Serialize>(value: &T) -> String {
    let pretty = serde_json::to_string_pretty(value).expect("the value serialises to JSON");
    escape_non_ascii(&pretty)
}

/// Escape every non-ASCII character and DEL (Python's `ensure_ascii` keeps only space to `~`). JSON
/// text holds them only inside strings, where `\uXXXX` is the same string.
#[must_use]
pub fn escape_non_ascii(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() && c != '\x7f' {
            out.push(c);
        } else {
            let mut units = [0_u16; 2];
            for unit in c.encode_utf16(&mut units) {
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct Sample {
        zeta: &'static str,
        alpha: Vec<u32>,
        none: Option<u32>,
    }

    #[test]
    fn non_ascii_becomes_escapes_and_pairs() {
        assert_eq!(
            escape_non_ascii("\"Größe 😀\""),
            "\"Gr\\u00f6\\u00dfe \\ud83d\\ude00\""
        );
    }

    #[test]
    fn del_is_escaped_like_python() {
        assert_eq!(escape_non_ascii("a\x7fb"), "a\\u007fb");
    }

    #[test]
    fn the_layout_is_pythons_indent_two() {
        let text = dumps_indent2(&Sample {
            zeta: "ä",
            alpha: vec![1, 2],
            none: None,
        });
        assert_eq!(
            text,
            "{\n  \"zeta\": \"\\u00e4\",\n  \"alpha\": [\n    1,\n    2\n  ],\n  \"none\": null\n}"
        );
        assert_eq!(
            dumps_indent2(&Sample {
                zeta: "",
                alpha: vec![],
                none: Some(3)
            }),
            "{\n  \"zeta\": \"\",\n  \"alpha\": [],\n  \"none\": 3\n}"
        );
    }
}

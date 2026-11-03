//! Python-compatible diagnostic redaction with bounded regex work.
use fancy_regex::{Captures, Regex, RegexBuilder};
use std::{collections::HashMap, sync::OnceLock};

fn patterns() -> &'static HashMap<String, Regex> {
    static PATTERNS: OnceLock<HashMap<String, Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let patterns: HashMap<String, String> =
            serde_json::from_str(include_str!("redaction_patterns.json"))
                .expect("checked-in redaction patterns");
        patterns
            .into_iter()
            .map(|(name, pattern)| {
                let regex = RegexBuilder::new(&pattern)
                    .backtrack_limit(50_000)
                    .build()
                    .expect("checked-in redaction regex");
                (name, regex)
            })
            .collect()
    })
}

fn substitute(
    value: &str,
    name: &str,
    mut replacement: impl FnMut(&Captures<'_, str>) -> String,
) -> Result<String, ()> {
    let mut output = String::new();
    let mut end = 0;
    for captures in patterns()[name].captures_iter(value) {
        let captures = captures.map_err(|_| ())?;
        let matched = captures.get(0).ok_or(())?;
        output.push_str(&value[end..matched.start()]);
        output.push_str(&replacement(&captures));
        end = matched.end();
    }
    output.push_str(&value[end..]);
    Ok(output)
}
fn group<'a>(captures: &Captures<'a, str>, name: &str) -> &'a str {
    captures.name(name).map_or("", |matched| matched.as_str())
}
fn absolute(value: &str) -> bool {
    let value = value.trim();
    if value.contains("://") {
        return false;
    }
    let bytes = value.as_bytes();
    value.starts_with('/')
        || value.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}
fn safe_url(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_owned();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(end);
    let authority = authority.rsplit_once('@').map_or_else(
        || authority.to_owned(),
        |(_, host)| format!("<redacted>@{host}"),
    );
    let (before_fragment, fragment) = rest
        .split_once('#')
        .map_or((rest, None), |(a, b)| (a, Some(b)));
    let mut output = format!("{}://{authority}", scheme.to_ascii_lowercase());
    if let Some((path, query)) = before_fragment.split_once('?') {
        output.push_str(path);
        if !query.is_empty() {
            output.push('?');
            for (index, field) in query.split('&').enumerate() {
                if index > 0 {
                    output.push('&');
                }
                if let Some((key, _)) = field.split_once('=') {
                    let normalized = key.to_ascii_lowercase().replace('-', "_");
                    if matches!(
                        normalized.as_str(),
                        "token"
                            | "secret"
                            | "password"
                            | "apikey"
                            | "api_key"
                            | "authorization"
                            | "credential"
                            | "cookie"
                            | "access_token"
                            | "refresh_token"
                            | "client_secret"
                    ) {
                        output.push_str(key);
                        output.push_str("=<redacted>");
                        continue;
                    }
                }
                output.push_str(field);
            }
        }
    } else {
        output.push_str(before_fragment);
    }
    if let Some(fragment) = fragment.filter(|value| !value.is_empty()) {
        output.push('#');
        output.push_str(fragment);
    }
    output
}
fn redact(value: &str) -> Result<String, ()> {
    if value.len() > 256 * 1024 {
        return Err(());
    }
    let mut urls = Vec::new();
    let mut value = substitute(value, "URL", |captures| {
        urls.push(safe_url(captures.get(0).unwrap().as_str()));
        format!("__REDACTION_URL_{}__", urls.len() - 1)
    })?;
    value = substitute(&value, "QUOTED_SECRET", |captures| {
        format!(
            "{}{}<redacted>{}",
            group(captures, "prefix"),
            group(captures, "quote"),
            group(captures, "quote")
        )
    })?;
    for name in ["AUTHORIZATION", "SECRET_TOKEN"] {
        value = substitute(&value, name, |captures| {
            format!("{}<redacted>", group(captures, "prefix"))
        })?;
    }
    value = substitute(&value, "PATH_KEY", |captures| {
        let path = captures
            .name("value")
            .or_else(|| captures.name("value_unquoted"))
            .map_or("", |value| value.as_str());
        if !absolute(path) {
            return captures.get(0).unwrap().as_str().to_owned();
        }
        let prefix = captures
            .name("prefix")
            .or_else(|| captures.name("prefix_unquoted"))
            .map_or("", |value| value.as_str());
        format!(
            "{prefix}{}<path>{}",
            group(captures, "quote"),
            group(captures, "quote")
        )
    })?;
    value = if absolute(&value) {
        "<path>".to_owned()
    } else {
        substitute(&value, "ABSOLUTE_PATH", |_| "<path>".to_owned())?
    };
    substitute(&value, "PLACEHOLDER", |captures| {
        captures
            .get(1)
            .and_then(|matched| matched.as_str().parse::<usize>().ok())
            .and_then(|index| urls.get(index))
            .cloned()
            .unwrap_or_else(|| "<redacted>".to_owned())
    })
}

/// Redact paths and credentials, retaining safe prose and sanitized URLs.
#[must_use]
pub fn redact_text(value: &str) -> String {
    redact(value).unwrap_or_else(|()| "<redacted>".to_owned())
}
/// Redact first, then retain at most `limit` UTF-8 bytes without a split character.
#[must_use]
pub fn redact_log_tail(value: &str, limit: usize) -> String {
    let redacted = redact_text(value);
    let mut start = redacted.len().saturating_sub(limit);
    while !redacted.is_char_boundary(start) {
        start += 1;
    }
    redacted[start..].to_owned()
}

/// Redact and bound structured inspection values before encoding.
#[must_use]
pub fn redact_inspection_value(value: &serde_json::Value, limit: usize) -> serde_json::Value {
    let bounded = crate::read_files::inspection_value(value, 0);
    if json_size(&bounded).is_none_or(|size| size > limit) {
        serde_json::json!({"truncated":true})
    } else {
        bounded
    }
}

/// Return a bounded event object, wrapping scalar payloads like the Python API.
#[must_use]
pub fn bounded_event_payload(value: &serde_json::Value) -> serde_json::Value {
    let bounded = crate::read_files::inspection_value(value, 0);
    let bounded = if bounded.is_object() {
        bounded
    } else {
        serde_json::json!({"value":bounded})
    };
    if json_size(&bounded).is_none_or(|size| size > 8192) {
        serde_json::json!({"truncated":true})
    } else {
        bounded
    }
}

// Python response bounds count the ASCII-escaped JSON representation.
fn json_size(value: &serde_json::Value) -> Option<usize> {
    let encoded = serde_json::to_string(value).ok()?;
    Some(
        encoded
            .chars()
            .map(|character| {
                if character.is_ascii() {
                    1
                } else if u32::from(character) <= 0xffff {
                    6
                } else {
                    12
                }
            })
            .sum(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hostile_text_fails_closed_and_utf8_tails_are_byte_bounded() {
        assert_eq!(redact_text(&"x".repeat(256 * 1024 + 1)), "<redacted>");
        assert_eq!(redact_log_tail("a😀é", 5), "é");
        let hostile = format!("prefix /{}", "x ".repeat(20_000));
        assert!(!redact_text(&hostile).contains("x "));
    }
}

//! Parsers for the text `mp4info` and `mp4dump` print.
//!
//! They follow the Python parsers line by line, including their quirks: only top-level `mp4info`
//! fields (two-space indent) count, the first occurrence of a key wins, and a line is a line the
//! way Python's `str.splitlines` sees it.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

/// The text was not in a shape the parser can read (Python raised an exception here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("the pattern is valid")
}

static TRACK_HEADER: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^Track\s+(\d+):\s*$"));
static FIELD: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s*([A-Za-z][A-Za-z0-9 _-]*?):\s*(.+?)\s*$"));
static FTYP_BRAND: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?m)^\s*compatible_brand\s*=\s*(\S+)\s*$"));
static AVC_LEVEL: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^\s*AVC Level:\s*(\d+)\s*$"));
static HEVC_LEVEL: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?m)^\s*HEVC Level:\s*([0-9.]+)\s*$"));
static CODING: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^\s*Coding:\s*(\S+)"));
static SAMPLE_RATE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^\s*Sample Rate:\s*(\d+)\s*$"));

/// `text.splitlines()` as Python does it: `\n`, `\r\n`, `\r`, `\x0b`, `\x0c`, `\x1c`-`\x1e`,
/// `\x85`, `\u2028` and `\u2029` end a line; a final line break adds no empty line.
#[must_use]
pub fn py_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if !matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            continue;
        }
        lines.push(&text[start..at]);
        start = at + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
            chars.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Parse `mp4info` output into one dictionary per `Track N:` block.
///
/// Keys are the top-level `key: value` pairs (lower-cased, spaces turned into underscores); the
/// first occurrence of a key wins. A line that mentions Dolby Atmos with `yes` adds `atmos = yes`.
///
/// # Errors
///
/// [`ParseError`] for an Atmos line without a colon (Python raised an `IndexError` there).
pub fn parse_mp4info_tracks(text: &str) -> Result<Vec<BTreeMap<String, String>>, ParseError> {
    let headers: Vec<_> = TRACK_HEADER.captures_iter(text).collect();
    let mut tracks = Vec::new();
    for (i, header) in headers.iter().enumerate() {
        let start = header.get(0).map_or(0, |m| m.start());
        let end = headers
            .get(i + 1)
            .and_then(|h| h.get(0))
            .map_or(text.len(), |m| m.start());
        let block = &text[start..end];
        let mut info = BTreeMap::from([("track_no".to_owned(), header[1].to_owned())]);
        let mut has_atmos = false;
        for line in py_splitlines(block).into_iter().skip(1) {
            if line.contains("Dolby Digital Plus with Dolby Atmos") {
                let (_, value) = line
                    .split_once(':')
                    .ok_or_else(|| ParseError("list index out of range".to_owned()))?;
                if value.trim().to_lowercase().starts_with("yes") {
                    has_atmos = true;
                }
                continue;
            }
            // Track attributes are indented by 2 spaces; nested blocks use 4 or more.
            if line.trim_end().is_empty() || indent_of(line) != 2 {
                continue;
            }
            let Some(field) = FIELD.captures(line) else {
                continue;
            };
            let key = field[1].trim().to_lowercase().replace(' ', "_");
            info.entry(key)
                .or_insert_with(|| field[2].trim().to_owned());
        }
        if has_atmos {
            info.insert("atmos".to_owned(), "yes".to_owned());
        }
        tracks.push(info);
    }
    Ok(tracks)
}

/// Whether bit 0 (ENABLED) is set in the `flags:` value, e.g. `3 ENABLED IN-MOVIE`.
///
/// The leading integer decides; when there is none the word `ENABLED` does.
#[must_use]
pub fn flags_enabled(flags: &str) -> bool {
    let Some(head) = flags.split_whitespace().next() else {
        return false;
    };
    let digits = head.strip_prefix(['+', '-']).unwrap_or(head);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        // The parity of the decimal number is the parity of its last digit (also for negatives).
        return digits.as_bytes().last().is_some_and(|d| d % 2 == 1);
    }
    flags.to_uppercase().contains("ENABLED")
}

/// The `compatible_brand` values of the first `[ftyp]` atom of an `mp4dump` output.
#[must_use]
pub fn parse_ftyp_brands(dump: &str) -> Vec<String> {
    let Some(at) = dump.find("[ftyp]") else {
        return Vec::new();
    };
    let after = &dump[at..];
    let block = after
        .get(1..)
        .and_then(|rest| rest.find("\n["))
        .map_or(after, |end| &after[..=end]);
    FTYP_BRAND
        .captures_iter(block)
        .map(|m| m[1].to_owned())
        .collect()
}

/// The first handler type of a `trak`, with the track's `tkhd` id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackHandler {
    /// 1-based order of appearance in the file.
    pub track_index: u32,
    /// `vide`, `soun`, `sbtl`, `text`, ...
    pub handler_type: String,
    /// `None` when `mp4dump` shows none (or one beyond `u64`).
    pub track_id: Option<u64>,
}

/// The first `handler_type` after each `[trak]` line, in document order.
#[must_use]
pub fn parse_track_handlers(dump: &str) -> Vec<TrackHandler> {
    let mut results = Vec::new();
    let (mut track_index, mut track_id, mut seen) = (0_u32, None, false);
    for line in py_splitlines(dump) {
        let stripped = line.trim();
        if stripped.starts_with("[trak]") {
            track_index += 1;
            track_id = None;
            seen = false;
            continue;
        }
        if seen || track_index == 0 {
            continue;
        }
        if track_id.is_none() && stripped.starts_with("id = ") {
            let value = stripped.split_once('=').map_or("", |(_, v)| v).trim();
            track_id = if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                value.parse().ok()
            } else {
                None
            };
            continue;
        }
        if stripped.starts_with("handler_type") {
            let value = stripped.split_once('=').map_or("", |(_, v)| v).trim();
            if !value.is_empty() {
                results.push(TrackHandler {
                    track_index,
                    handler_type: value.to_owned(),
                    track_id,
                });
                seen = true;
            }
        }
    }
    results
}

/// `(track id or number, block)` for each `Track N:` block.
fn split_track_blocks(text: &str) -> Vec<(String, &str)> {
    let headers: Vec<_> = TRACK_HEADER.captures_iter(text).collect();
    let mut blocks = Vec::new();
    for (i, header) in headers.iter().enumerate() {
        let start = header.get(0).map_or(0, |m| m.start());
        let end = headers
            .get(i + 1)
            .and_then(|h| h.get(0))
            .map_or(text.len(), |m| m.start());
        let block = &text[start..end];
        let mut track_id = header[1].to_owned();
        for line in py_splitlines(block) {
            if indent_of(line) == 2 && line.trim_end().trim_start().starts_with("id:") {
                let value = line
                    .trim_end()
                    .split_once(':')
                    .map_or("", |(_, v)| v)
                    .trim();
                if !value.is_empty() {
                    value.clone_into(&mut track_id);
                }
                break;
            }
        }
        blocks.push((track_id, block));
    }
    blocks
}

/// Whether the first `type:` line of the block (indent 2) says `kind`.
fn block_type_is(block: &str, kind: &str) -> bool {
    for line in py_splitlines(block) {
        if indent_of(line) == 2
            && line
                .trim_end()
                .trim_start()
                .to_lowercase()
                .starts_with("type:")
        {
            let value = line
                .trim_end()
                .split_once(':')
                .map_or("", |(_, v)| v)
                .trim()
                .to_lowercase();
            return value == kind;
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoCodecInfo {
    pub track_id: String,
    /// `avc1`, `hvc1`, `hev1`, ...
    pub coding: String,
    /// The printed level: `52` for AVC, `5.2` for HEVC.
    pub level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioCodecInfo {
    pub track_id: String,
    /// `mp4a`, `ac-3`, `ec-3`, ...
    pub coding: String,
    /// `None` when absent or beyond `u128`.
    pub sample_rate: Option<u128>,
}

/// One entry per video track in document order.
#[must_use]
pub fn parse_video_codec_infos(text: &str) -> Vec<VideoCodecInfo> {
    split_track_blocks(text)
        .into_iter()
        .filter(|(_, block)| block_type_is(block, "video"))
        .map(|(track_id, block)| {
            let coding = CODING
                .captures(block)
                .map_or_else(String::new, |m| m[1].to_lowercase());
            let level = match coding.as_str() {
                "avc1" | "avc3" => AVC_LEVEL.captures(block).map(|m| m[1].to_owned()),
                "hvc1" | "hev1" | "dvh1" | "dvhe" => {
                    HEVC_LEVEL.captures(block).map(|m| m[1].to_owned())
                }
                _ => None,
            };
            VideoCodecInfo {
                track_id,
                coding,
                level,
            }
        })
        .collect()
}

/// One entry per audio track in document order.
#[must_use]
pub fn parse_audio_codec_infos(text: &str) -> Vec<AudioCodecInfo> {
    split_track_blocks(text)
        .into_iter()
        .filter(|(_, block)| block_type_is(block, "audio"))
        .map(|(track_id, block)| AudioCodecInfo {
            track_id,
            coding: CODING
                .captures(block)
                .map_or_else(String::new, |m| m[1].to_lowercase()),
            sample_rate: SAMPLE_RATE.captures(block).and_then(|m| m[1].parse().ok()),
        })
        .collect()
}

/// Whether the printed HEVC level (`5.2`, `6.0`) is above 5.2; text that is no number says no.
#[must_use]
pub fn hevc_level_exceeds_52(level: &str) -> bool {
    level.parse::<f64>().is_ok_and(|n| n > 5.2)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    fn texts() -> BTreeMap<String, String> {
        #[derive(Deserialize)]
        struct Ignored {}
        load_golden::<Ignored, String>("compliance_texts")
            .into_iter()
            .map(|c| (c.name, c.expected))
            .collect()
    }

    #[derive(Deserialize)]
    struct Input {
        parser: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        value: String,
    }

    fn error_json(error: &ParseError) -> Value {
        json!({"raises": "IndexError", "message": error.0})
    }

    // The golden float for a rate beyond u64 is compared against the integer's nearest float.
    #[allow(clippy::cast_precision_loss)]
    #[test]
    fn parsers_match_the_python_oracle() {
        let texts = texts();
        let cases = load_golden::<Input, Value>("compliance_parse");
        assert!(cases.len() > 650);
        for case in cases {
            let input = &case.input;
            let text = texts
                .get(&input.text)
                .map(String::as_str)
                .unwrap_or_default();
            let actual = match input.parser.as_str() {
                "tracks" => match parse_mp4info_tracks(text) {
                    Ok(tracks) => json!(tracks),
                    Err(error) => error_json(&error),
                },
                "video" => json!(
                    parse_video_codec_infos(text)
                        .iter()
                        .map(|i| json!({"track_id": i.track_id, "coding": i.coding, "level": i.level}))
                        .collect::<Vec<_>>()
                ),
                "audio" => json!(
                    parse_audio_codec_infos(text)
                        .iter()
                        // The one JSON number beyond u64 in the golden arrives as a float; compare as one.
                        .map(|i| json!({"track_id": i.track_id, "coding": i.coding, "sample_rate": i.sample_rate.map(|n| n as f64)}))
                        .collect::<Vec<_>>()
                ),
                "brands" => json!(parse_ftyp_brands(text)),
                "handlers" => json!(
                    parse_track_handlers(text)
                        .iter()
                        .map(|h| json!({"track_index": h.track_index, "handler_type": h.handler_type, "track_id": h.track_id}))
                        .collect::<Vec<_>>()
                ),
                "flags" => json!(flags_enabled(&input.value)),
                "level" => json!(hevc_level_exceeds_52(&input.value)),
                "lines" => json!(py_splitlines(&input.value)),
                other => panic!("unknown parser {other}"),
            };
            if input.parser == "audio" {
                // The golden float is the rounded value of the integer: compare through f64.
                assert_eq!(
                    normalise(&actual),
                    normalise(&case.expected),
                    "{}",
                    case.name
                );
            } else {
                assert_eq!(actual, case.expected, "{}", case.name);
            }
        }
    }

    fn normalise(value: &Value) -> Value {
        match value {
            Value::Array(items) => Value::Array(items.iter().map(normalise).collect()),
            Value::Object(map) => {
                Value::Object(map.iter().map(|(k, v)| (k.clone(), normalise(v))).collect())
            }
            Value::Number(n) => json!(n.as_f64()),
            other => other.clone(),
        }
    }

    #[test]
    fn a_text_without_track_headers_gives_no_tracks() {
        assert_eq!(
            parse_mp4info_tracks("File:\n  major brand: isom\n")
                .unwrap()
                .len(),
            0
        );
        assert_eq!(parse_video_codec_infos("nothing").len(), 0);
    }

    #[test]
    fn splitlines_splits_where_python_does() {
        assert_eq!(
            py_splitlines("a\r\nb\rc\x0bd\u{2028}e"),
            ["a", "b", "c", "d", "e"]
        );
        assert_eq!(py_splitlines("a\n"), ["a"]);
        assert_eq!(py_splitlines("a\n\nb"), ["a", "", "b"]);
    }

    #[test]
    fn huge_numbers_do_not_panic() {
        let text = "Track 1:\n  type: Audio\n  Sample Rate: 999999999999999999999999999999999999999999999\n";
        assert_eq!(parse_audio_codec_infos(text).len(), 1);
        assert!(hevc_level_exceeds_52("99999999999999999999999999999.0"));
        assert!(flags_enabled("99999999999999999999999999999999999999999"));
    }
}

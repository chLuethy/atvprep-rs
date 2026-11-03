//! Fixed tables and the language helpers shared by every layer (port of `atvprep/constants.py`).

/// Container extensions atvprep treats as video.
pub const VIDEO_EXTS: &[&str] = &[
    ".avi", ".m2ts", ".m4v", ".mkv", ".mov", ".mp4", ".ts", ".webm",
];

/// Subtitle codecs that convert to `mov_text`.
pub const TEXT_SUB_CODECS: &[&str] = &["mov_text", "srt", "subrip", "webvtt"];

/// Bitmap subtitle codecs, skipped (not MP4-compatible).
pub const IMAGE_SUB_CODECS: &[&str] = &["dvd_subtitle", "hdmv_pgs_subtitle"];

/// Video codecs that always transcode (AV1 is handled by `--keep-av1`).
pub const FORCE_TRANSCODE_VIDEO: &[&str] = &["vc1", "vp9"];

/// H.264 profiles the Apple TV decodes in hardware (8-bit only).
pub const ATV_H264_OK_PROFILES: &[&str] = &[
    "baseline",
    "constrained baseline",
    "constrained high",
    "extended",
    "high",
    "main",
];

/// HEVC profiles the Apple TV decodes in hardware.
pub const ATV_HEVC_OK_PROFILES: &[&str] = &["main", "main 10", "main still picture"];

/// AV1 profiles with hardware decode; empty means ffprobe reported none.
pub const ATV_AV1_OK_PROFILES: &[&str] = &["", "main"];

/// Audio codecs copied as they are.
pub const COMPAT_AUDIO_COPY: &[&str] = &["ac3", "eac3"];

/// Audio codecs transcoded to E-AC-3.
pub const TRANSCODE_TO_EAC3: &[&str] = &["dca", "dts", "flac", "mlp", "opus", "truehd", "vorbis"];

/// Pixel formats the Apple TV decodes for H.264 in hardware.
pub const ATV_H264_OK_PIX_FMTS: &[&str] = &["yuv420p", "yuvj420p"];

/// Pixel formats the Apple TV decodes for HEVC in hardware.
pub const ATV_HEVC_OK_PIX_FMTS: &[&str] = &["yuv420p", "yuv420p10le", "yuvj420p"];

/// Language tag to ISO 639-2 code.
pub const LANG_MAP: &[(&str, &str)] = &[
    ("de", "deu"),
    ("ger", "deu"),
    ("deu", "deu"),
    ("en", "eng"),
    ("eng", "eng"),
    ("fr", "fra"),
    ("fre", "fra"),
    ("fra", "fra"),
    ("it", "ita"),
    ("ita", "ita"),
    ("es", "spa"),
    ("spa", "spa"),
    ("pt", "por"),
    ("por", "por"),
    ("nl", "nld"),
    ("dut", "nld"),
    ("nld", "nld"),
    ("ko", "kor"),
    ("kor", "kor"),
    ("ja", "jpn"),
    ("jpn", "jpn"),
    ("zh", "zho"),
    ("chi", "zho"),
    ("zho", "zho"),
    ("hi", "hin"),
    ("hin", "hin"),
    ("ru", "rus"),
    ("rus", "rus"),
    ("tr", "tur"),
    ("tur", "tur"),
    ("ar", "ara"),
    ("ara", "ara"),
    ("pl", "pol"),
    ("pol", "pol"),
    ("sv", "swe"),
    ("swe", "swe"),
    ("no", "nor"),
    ("nor", "nor"),
    ("da", "dan"),
    ("dan", "dan"),
    ("fi", "fin"),
    ("fin", "fin"),
    ("th", "tha"),
    ("tha", "tha"),
    ("vi", "vie"),
    ("vie", "vie"),
    ("bn", "ben"),
    ("ben", "ben"),
    ("pa", "pan"),
    ("pan", "pan"),
    ("ta", "tam"),
    ("tam", "tam"),
    ("te", "tel"),
    ("tel", "tel"),
];

/// ISO 639-2 code to English display name.
pub const HANDLER_NAMES: &[(&str, &str)] = &[
    ("deu", "German"),
    ("eng", "English"),
    ("fra", "French"),
    ("ita", "Italian"),
    ("spa", "Spanish"),
    ("por", "Portuguese"),
    ("nld", "Dutch"),
    ("kor", "Korean"),
    ("jpn", "Japanese"),
    ("zho", "Chinese"),
    ("hin", "Hindi"),
    ("rus", "Russian"),
    ("tur", "Turkish"),
    ("ara", "Arabic"),
    ("pol", "Polish"),
    ("swe", "Swedish"),
    ("nor", "Norwegian"),
    ("dan", "Danish"),
    ("fin", "Finnish"),
    ("tha", "Thai"),
    ("vie", "Vietnamese"),
    ("ben", "Bengali"),
    ("pan", "Punjabi"),
    ("tam", "Tamil"),
    ("tel", "Telugu"),
];

/// Characters Python's `str.strip` removes that Unicode `White_Space` does not list.
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Normalize a language tag to an ISO 639-2 three-letter code; unknown tags stay lowercase as given.
#[must_use]
pub fn normalize_lang(lang: Option<&str>) -> String {
    let Some(raw) = lang else {
        return "und".to_owned();
    };
    let s = raw.to_lowercase();
    let s = s.trim_matches(is_python_space);
    if s.is_empty() {
        return "und".to_owned();
    }
    LANG_MAP
        .iter()
        .find(|&&(key, _)| key == s)
        .map_or_else(|| s.to_owned(), |&(_, code)| code.to_owned())
}

/// English display name for a language code, or the code itself.
#[must_use]
pub fn handler_name(lang: &str) -> String {
    HANDLER_NAMES
        .iter()
        .find(|&&(code, _)| code == lang)
        .map_or_else(|| lang.to_owned(), |&(_, name)| name.to_owned())
}

#[cfg(test)]
mod tests {
    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::Value;

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        value: Value,
    }

    fn sorted(items: &[&str]) -> Value {
        let mut v: Vec<&str> = items.to_vec();
        v.sort_unstable();
        serde_json::to_value(v).unwrap()
    }

    fn table(pairs: &[(&str, &str)]) -> Value {
        let mut v: Vec<[&str; 2]> = pairs.iter().map(|&(k, n)| [k, n]).collect();
        v.sort_unstable();
        serde_json::to_value(v).unwrap()
    }

    #[test]
    fn matches_the_python_oracle() {
        for case in load_golden::<Input, Value>("lang") {
            let actual = match case.input.kind.as_str() {
                "normalize" => Value::from(normalize_lang(case.input.value.as_str())),
                "handler" => Value::from(handler_name(case.input.value.as_str().unwrap())),
                "set" => match case.input.value.as_str().unwrap() {
                    "VIDEO_EXTS" => sorted(VIDEO_EXTS),
                    "TEXT_SUB_CODECS" => sorted(TEXT_SUB_CODECS),
                    "IMAGE_SUB_CODECS" => sorted(IMAGE_SUB_CODECS),
                    "FORCE_TRANSCODE_VIDEO" => sorted(FORCE_TRANSCODE_VIDEO),
                    "ATV_H264_OK_PROFILES" => sorted(ATV_H264_OK_PROFILES),
                    "ATV_HEVC_OK_PROFILES" => sorted(ATV_HEVC_OK_PROFILES),
                    "ATV_AV1_OK_PROFILES" => sorted(ATV_AV1_OK_PROFILES),
                    "COMPAT_AUDIO_COPY" => sorted(COMPAT_AUDIO_COPY),
                    "TRANSCODE_TO_EAC3" => sorted(TRANSCODE_TO_EAC3),
                    "_ATV_H264_OK_PIX_FMTS" => sorted(ATV_H264_OK_PIX_FMTS),
                    "_ATV_HEVC_OK_PIX_FMTS" => sorted(ATV_HEVC_OK_PIX_FMTS),
                    other => panic!("unknown set {other}"),
                },
                "table" => match case.input.value.as_str().unwrap() {
                    "LANG_MAP" => table(LANG_MAP),
                    "HANDLER_NAMES" => table(HANDLER_NAMES),
                    other => panic!("unknown table {other}"),
                },
                other => panic!("unknown kind {other}"),
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn missing_or_blank_language_is_undetermined() {
        assert_eq!(normalize_lang(None), "und");
        assert_eq!(normalize_lang(Some("  ")), "und");
    }

    #[test]
    fn unknown_codes_stay_as_they_are() {
        assert_eq!(normalize_lang(Some("pt-BR")), "pt-br");
        assert_eq!(handler_name("xx"), "xx");
    }
}

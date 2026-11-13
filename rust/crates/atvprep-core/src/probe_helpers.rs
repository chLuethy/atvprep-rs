//! Pure helpers over ffprobe JSON that more than one module needs (from `atvprep/probe.py`).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::constants::{
    ATV_H264_OK_PIX_FMTS, ATV_H264_OK_PROFILES, ATV_HEVC_OK_PIX_FMTS, ATV_HEVC_OK_PROFILES,
    normalize_lang,
};
use crate::ffprobe::Ffprobe;
use crate::mp4::patch::is_faststart;

const FORCED_TITLE_HINTS: [&str; 3] = ["forced", ".f.", "gezwungen"];

/// A title that mentions forced but names a full track: "Non-Forced", "ohne Forced", "Full + Forced".
static FULL_TRACK_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:non|not|no|un|ohne|keine?|nicht)[\s._-]*(?:forced|gezwungen)|\b(?:full|complete|komplett|vollständig)\b",
    )
    .expect("the full-track pattern is valid")
});

/// Python truthiness of a JSON value, as `bool(x)` would see it.
#[must_use]
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_none_or(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// An object-valued key, or `None` when absent, null or not an object.
#[must_use]
pub fn object<'a>(value: &'a Value, key: &str) -> Option<&'a serde_json::Map<String, Value>> {
    value.get(key).and_then(Value::as_object)
}

/// True if the subtitle stream is flagged forced via disposition or a forced hint in its title.
///
/// A title that negates the hint or calls the track full does not count: marked forced, a full
/// track becomes forced-only, shown with its audio and impossible to switch off.
#[must_use]
pub fn subtitle_is_forced(stream: &Value) -> bool {
    if truthy(object(stream, "disposition").and_then(|d| d.get("forced"))) {
        return true;
    }
    let title = object(stream, "tags")
        .and_then(|t| t.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    FORCED_TITLE_HINTS.iter().any(|hint| title.contains(hint)) && !FULL_TRACK_TITLE.is_match(&title)
}

/// True if the subtitle stream is SDH: the `hearing_impaired` disposition or an SDH hint in its
/// title.
///
/// The one place SDH is read from a stream. The disposition matters for MP4: its muxer drops
/// subtitle titles, so a prepared SDH track keeps only the flag (#62).
#[must_use]
pub fn subtitle_is_sdh(stream: &Value) -> bool {
    if truthy(object(stream, "disposition").and_then(|d| d.get("hearing_impaired"))) {
        return true;
    }
    let title = object(stream, "tags")
        .and_then(|t| t.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    ["sdh", " hi ", "hearing", "hörgeschädigt"]
        .iter()
        .any(|x| title.contains(x))
}

/// Cached per-file probe results, loaded once so the planner does not shell out again.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeContext {
    pub infile: PathBuf,
    pub ffprobe: Value,
    pub first_video_dts: Option<i64>,
    pub is_faststart: bool,
}

impl ProbeContext {
    /// Probe `infile` once and populate every field.
    ///
    /// Faststart is only checked for mp4-like containers (`.mp4`, `.m4v`, `.mov`); for other
    /// extensions the field is `false`. On any probe failure (for example the file does not exist)
    /// the ffprobe result defaults to `{}` and the DTS to `None` instead of failing, so callers
    /// that only need DTS gating can still build a context.
    #[must_use]
    pub fn load(infile: &Path, ffprobe: &Ffprobe) -> Self {
        let probe = ffprobe
            .probe_json(infile)
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
        let mp4_like = infile
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| {
                ["mp4", "m4v", "mov"]
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(e))
            });
        Self {
            infile: infile.to_path_buf(),
            ffprobe: probe,
            first_video_dts: ffprobe.first_video_dts(infile),
            is_faststart: mp4_like && is_faststart(infile).unwrap_or(false),
        }
    }
}

/// `str(v).lower()` for substring tests: strings as they are, other values in their JSON form.
#[must_use]
pub fn py_str_lower(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_lowercase(),
        other => other.to_string().to_lowercase(),
    }
}

/// `int(value or 0)` for JSON numbers, booleans and numeric strings; anything else counts as 0
/// (Python would raise).
#[must_use]
pub fn py_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Bool(b)) => i64::from(*b),
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(float_to_int))
            .unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Python's `int(float)`: truncation toward zero, saturating at the `i64` range.
#[allow(clippy::cast_possible_truncation)]
fn float_to_int(f: f64) -> i64 {
    f.trunc() as i64
}

/// `(stream.get(key) or "").lower()`; non-string values count as empty.
fn lower_field(stream: &Value, key: &str) -> String {
    stream
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase()
}

fn side_data(stream: &Value) -> &[Value] {
    stream
        .get("side_data_list")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

fn mentions_dolby_vision(side: &Value) -> bool {
    let dumped = side.to_string().to_lowercase();
    dumped.contains("dolby vision") || dumped.contains("dovi")
}

#[must_use]
pub fn is_hdr(stream: &Value) -> bool {
    let transfer = lower_field(stream, "color_transfer");
    matches!(transfer.as_str(), "smpte2084" | "arib-std-b67")
        || lower_field(stream, "color_primaries") == "bt2020"
        || lower_field(stream, "color_space").contains("bt2020")
}

#[must_use]
pub fn is_dolby_vision(stream: &Value) -> bool {
    if let Some(tags) = object(stream, "tags") {
        if tags.keys().any(|k| k.to_lowercase().contains("dovi")) {
            return true;
        }
        if tags.values().any(|v| py_str_lower(v).contains("dovi")) {
            return true;
        }
    }
    side_data(stream).iter().any(mentions_dolby_vision)
}

/// The Dolby Vision profile number from `side_data_list`, if any entry names one.
#[must_use]
pub fn dolby_vision_profile(stream: &Value) -> Option<i64> {
    side_data(stream)
        .iter()
        .filter(|sd| mentions_dolby_vision(sd))
        .find_map(|sd| match sd.get("dv_profile")? {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
}

/// The MP4 sample-entry tag for an HEVC stream: `dvh1` for Dolby Vision Profile 5, else `hvc1`.
///
/// Profile 5 has no HDR10 or SDR base layer, so a player that is not told "this is Dolby Vision"
/// shows it with wrong colours.
#[must_use]
pub fn hevc_sample_tag(stream: &Value) -> &'static str {
    if dolby_vision_profile(stream) == Some(5) {
        "dvh1"
    } else {
        "hvc1"
    }
}

/// True only when the bitstream metadata (`profile`, `side_data_list`) shows Atmos/JOC.
///
/// Tag text such as a track title is ignored on purpose: a false positive keeps the `dby1` brand on
/// non-Atmos audio, which makes AVRs play silence, while a false negative only loses the badge.
#[must_use]
pub fn is_atmos_audio(stream: &Value) -> bool {
    if lower_field(stream, "codec_name") != "eac3" {
        return false;
    }
    let mut haystack = Vec::new();
    if let Some(profile) = stream.get("profile").and_then(Value::as_str) {
        haystack.push(profile.to_owned());
    }
    let sdl = side_data(stream);
    if !sdl.is_empty() {
        haystack.push(Value::Array(sdl.to_vec()).to_string());
    }
    let blob = haystack.join(" ").to_lowercase();
    blob.contains("atmos") || blob.contains("joc")
}

fn profile_and_pix_fmt(stream: &Value) -> (String, String) {
    (
        lower_field(stream, "profile").trim().to_owned(),
        lower_field(stream, "pix_fmt"),
    )
}

/// True if this H.264 stream can be hardware-decoded on Apple TV (safe to copy).
#[must_use]
pub fn h264_is_atv_compatible(stream: &Value) -> bool {
    let (profile, pix_fmt) = profile_and_pix_fmt(stream);
    let profile = match profile.as_str() {
        "66" => "baseline",
        "77" => "main",
        "88" => "extended",
        "100" => "high",
        other => other,
    };
    if !pix_fmt.is_empty() && !ATV_H264_OK_PIX_FMTS.contains(&pix_fmt.as_str()) {
        return false;
    }
    profile.is_empty() || ATV_H264_OK_PROFILES.contains(&profile)
}

/// True if this HEVC stream can be hardware-decoded on Apple TV (safe to copy).
#[must_use]
pub fn hevc_is_atv_compatible(stream: &Value) -> bool {
    let (profile, pix_fmt) = profile_and_pix_fmt(stream);
    if !pix_fmt.is_empty() && !ATV_HEVC_OK_PIX_FMTS.contains(&pix_fmt.as_str()) {
        return false;
    }
    profile.is_empty() || ATV_HEVC_OK_PROFILES.contains(&profile.as_str())
}

/// `(lang, is_sdh, is_forced)` for each subtitle stream in `probe`, in file order.
#[must_use]
pub fn subtitle_infos(probe: &Value) -> Vec<(String, bool, bool)> {
    let streams = probe
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    streams
        .iter()
        .filter(|s| s.get("codec_type").and_then(Value::as_str) == Some("subtitle"))
        .map(|s| {
            let lang = normalize_lang(
                object(s, "tags")
                    .and_then(|t| t.get("language"))
                    .and_then(Value::as_str),
            );
            (lang, subtitle_is_sdh(s), subtitle_is_forced(s))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn forced(title: &str, disposition: &serde_json::Value) -> bool {
        subtitle_is_forced(&json!({"tags": {"title": title}, "disposition": disposition}))
    }

    #[test]
    fn the_disposition_flag_alone_marks_a_track_forced() {
        assert!(forced("Full", &json!({"forced": 1})));
        assert!(forced("Forced", &json!({"forced": 0})));
    }

    #[test]
    fn a_title_hint_marks_a_track_forced_unless_it_names_a_full_track() {
        assert!(forced("Deutsch (gezwungen)", &json!({})));
        assert!(!forced("Non-Forced", &json!({})));
        assert!(!forced("Full + Forced", &json!({})));
    }

    #[test]
    fn missing_or_null_tags_and_disposition_mean_not_forced() {
        assert!(!subtitle_is_forced(&json!({})));
        assert!(!subtitle_is_forced(
            &json!({"tags": null, "disposition": null})
        ));
    }

    #[derive(serde::Deserialize)]
    struct Input {
        #[serde(rename = "fn")]
        func: String,
        arg: Value,
    }

    #[test]
    fn helpers_match_the_python_oracle() {
        let cases = atvprep_testkit::load_golden::<Input, Value>("probe_helpers");
        assert!(cases.len() > 200);
        for case in cases {
            let arg = &case.input.arg;
            let actual = match case.input.func.as_str() {
                "is_hdr" => Value::from(is_hdr(arg)),
                "is_dolby_vision" => Value::from(is_dolby_vision(arg)),
                "dolby_vision_profile" => Value::from(dolby_vision_profile(arg)),
                "hevc_sample_tag" => Value::from(hevc_sample_tag(arg)),
                "is_atmos_audio" => Value::from(is_atmos_audio(arg)),
                "h264_is_atv_compatible" => Value::from(h264_is_atv_compatible(arg)),
                "hevc_is_atv_compatible" => Value::from(hevc_is_atv_compatible(arg)),
                "subtitle_infos" => json!(
                    subtitle_infos(arg)
                        .into_iter()
                        .map(|(l, s, f)| json!([l, s, f]))
                        .collect::<Vec<_>>()
                ),
                other => panic!("unknown fn {other}"),
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn atmos_is_not_inferred_from_the_title() {
        let stream = json!({"codec_name": "eac3", "tags": {"title": "Atmos 7.1"}});
        assert!(!is_atmos_audio(&stream));
    }

    #[test]
    fn profile_5_gets_the_dvh1_tag() {
        let stream = json!({"side_data_list": [{"side_data_type": "DOVI configuration record", "dv_profile": 5}]});
        assert_eq!(hevc_sample_tag(&stream), "dvh1");
        assert_eq!(hevc_sample_tag(&json!({})), "hvc1");
    }

    #[cfg(unix)]
    mod load {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::Path;

        use atvprep_testkit::fixture_path;
        use serde_json::json;

        use super::super::*;
        use crate::ffprobe::Ffprobe;

        /// A fake ffprobe: JSON for the stream probe, one packet for the DTS probe, or a failure.
        fn fake(dir: &Path, body: &str) -> Ffprobe {
            let script = dir.join("ffprobe");
            fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            Ffprobe::with_program(script)
        }

        const WORKING: &str = r#"case "$*" in *-show_format*) printf '{"streams": [], "format": {"duration": "1"}}';; *) printf '{"packets": [{"dts": "-5"}]}';; esac"#;

        #[test]
        fn load_fills_all_fields_for_an_mp4() {
            let dir = tempfile::tempdir().unwrap();
            let video = dir.path().join("film.MP4");
            fs::copy(fixture_path("plain_faststart.mp4"), &video).unwrap();
            let context = ProbeContext::load(&video, &fake(dir.path(), WORKING));
            assert_eq!(context.infile, video);
            assert_eq!(
                context.ffprobe,
                json!({"streams": [], "format": {"duration": "1"}})
            );
            assert_eq!(context.first_video_dts, Some(-5));
            assert!(context.is_faststart);
        }

        #[test]
        fn load_survives_a_failing_ffprobe() {
            let dir = tempfile::tempdir().unwrap();
            let video = dir.path().join("film.mp4");
            fs::copy(fixture_path("plain_faststart.mp4"), &video).unwrap();
            let context = ProbeContext::load(&video, &fake(dir.path(), "exit 1"));
            assert_eq!(context.ffprobe, json!({}));
            assert_eq!(context.first_video_dts, None);
            assert!(
                context.is_faststart,
                "the layout is read from the file, not from ffprobe"
            );
            let missing = ProbeContext::load(
                &dir.path().join("nope.mp4"),
                &Ffprobe::with_program("/definitely/not/ffprobe"),
            );
            assert_eq!(
                (
                    missing.ffprobe,
                    missing.first_video_dts,
                    missing.is_faststart
                ),
                (json!({}), None, false)
            );
        }

        #[test]
        fn faststart_is_false_for_other_extensions_and_unreadable_files() {
            let dir = tempfile::tempdir().unwrap();
            let mkv = dir.path().join("film.mkv");
            fs::copy(fixture_path("plain_faststart.mp4"), &mkv).unwrap();
            let ffprobe = fake(dir.path(), WORKING);
            assert!(!ProbeContext::load(&mkv, &ffprobe).is_faststart);
            let moov_last = dir.path().join("last.m4v");
            fs::copy(fixture_path("plain_moov_last.mp4"), &moov_last).unwrap();
            assert!(!ProbeContext::load(&moov_last, &ffprobe).is_faststart);
            assert!(!ProbeContext::load(&dir.path().join("gone.mov"), &ffprobe).is_faststart);
        }
    }
}

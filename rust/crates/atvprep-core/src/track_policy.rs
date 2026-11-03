//! Track-selection policy: which tracks carry the `tkhd` ENABLED flag (port of `atvprep/track_policy.py`).
//!
//! The policy reproduces what Apple's own iTunes files do:
//!
//! - Exactly one ENABLED video track; the JPEG chapter track is disabled.
//! - Exactly one ENABLED audio track: the stereo AAC in the original language. The surround track
//!   of the same language is disabled and carries a `tref/fall` reference to the stereo track.
//! - Exactly one ENABLED subtitle track: the forced-only track in the enabled audio's language.
//!   The full track of that language carries a `tref/forc` reference to it.
//!
//! On MP4, ffprobe's `disposition.default` equals the `tkhd` ENABLED bit and `stream["id"]` is the
//! track id as a hex string.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::constants::normalize_lang;
use crate::probe_helpers::{object, subtitle_is_forced, truthy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Media {
    Video,
    Audio,
    Subtitle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrackRole {
    pub track_id: u32,
    pub media: Media,
    /// Normalized ISO 639-2, `und` when missing.
    pub lang: String,
    /// ffprobe `disposition.default` (== `tkhd` ENABLED on MP4).
    pub default: bool,
    /// Subtitles only; false for video and audio.
    pub forced: bool,
    /// Audio only; 0 otherwise.
    pub channels: u32,
}

/// The track id of a stream: ffprobe's hex string. Ids that do not fit a `tkhd` track id (u32) are
/// treated as unparsable.
fn parse_track_id(stream: &Value) -> Option<u32> {
    let text = match stream.get("id")? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let text = text.trim();
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    u32::from_str_radix(digits, 16).ok()
}

/// Build a [`TrackRole`] per video/audio/subtitle stream, skipping cover art and unparsable ids.
#[must_use]
pub fn roles_from_probe(probe: &Value) -> Vec<TrackRole> {
    let streams = probe
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut roles = Vec::new();
    for stream in streams {
        let media = match stream.get("codec_type").and_then(Value::as_str) {
            Some("video") => Media::Video,
            Some("audio") => Media::Audio,
            Some("subtitle") => Media::Subtitle,
            _ => continue,
        };
        let Some(track_id) = parse_track_id(stream) else {
            continue;
        };
        let disposition = object(stream, "disposition");
        if media == Media::Video && truthy(disposition.and_then(|d| d.get("attached_pic"))) {
            continue;
        }
        let language = object(stream, "tags")
            .and_then(|t| t.get("language"))
            .and_then(Value::as_str);
        let channels = match (media, stream.get("channels").and_then(Value::as_u64)) {
            (Media::Audio, Some(n)) => u32::try_from(n).unwrap_or(0),
            _ => 0,
        };
        roles.push(TrackRole {
            track_id,
            media,
            lang: normalize_lang(language),
            default: truthy(disposition.and_then(|d| d.get("default"))),
            forced: media == Media::Subtitle && subtitle_is_forced(stream),
            channels,
        });
    }
    roles
}

/// The track ids that should be ENABLED, following Apple's layout.
#[must_use]
pub fn enabled_track_ids(roles: &[TrackRole]) -> BTreeSet<u32> {
    let mut enabled = BTreeSet::new();
    if let Some(video) = roles.iter().find(|r| r.media == Media::Video) {
        enabled.insert(video.track_id);
    }

    let audios: Vec<&TrackRole> = roles.iter().filter(|r| r.media == Media::Audio).collect();
    let Some(&first) = audios.first() else {
        return enabled;
    };
    let base = audios.iter().find(|r| r.default).copied().unwrap_or(first);
    let audio = audios
        .iter()
        .find(|r| r.lang == base.lang && r.channels <= 2)
        .copied()
        .unwrap_or(base);
    enabled.insert(audio.track_id);

    if audio.lang != "und" {
        let forced = roles
            .iter()
            .find(|r| r.media == Media::Subtitle && r.forced && r.lang == audio.lang);
        if let Some(sub) = forced {
            enabled.insert(sub.track_id);
        }
    }
    enabled
}

/// The kind of track in words, for messages: `deu stereo audio`, `eng forced subtitle`.
#[must_use]
pub fn describe_role(role: &TrackRole) -> String {
    match role.media {
        Media::Video => "video".to_owned(),
        Media::Audio => {
            let layout = match role.channels {
                1 => "mono".to_owned(),
                2 => "stereo".to_owned(),
                6 => "5.1".to_owned(),
                8 => "7.1".to_owned(),
                0 => String::new(),
                n => format!("{n}-channel"),
            };
            [role.lang.as_str(), layout.as_str(), "audio"]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }
        Media::Subtitle => {
            let kind = if role.forced {
                "forced subtitle"
            } else {
                "full subtitle"
            };
            format!("{} {kind}", role.lang)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Deserialize)]
    struct Expected {
        roles: Vec<Value>,
        enabled: BTreeSet<u32>,
        described: Vec<String>,
    }

    #[test]
    fn policy_matches_the_python_oracle() {
        for case in load_golden::<Value, Expected>("track_policy") {
            let roles = roles_from_probe(&case.input);
            let actual: Vec<Value> = roles
                .iter()
                .map(|r| serde_json::to_value(r).unwrap())
                .collect();
            assert_eq!(actual, case.expected.roles, "{} roles", case.name);
            assert_eq!(
                enabled_track_ids(&roles),
                case.expected.enabled,
                "{} enabled",
                case.name
            );
            let described: Vec<String> = roles.iter().map(describe_role).collect();
            assert_eq!(
                described, case.expected.described,
                "{} described",
                case.name
            );
        }
    }

    fn role(
        track_id: u32,
        media: Media,
        lang: &str,
        default: bool,
        forced: bool,
        channels: u32,
    ) -> TrackRole {
        TrackRole {
            track_id,
            media,
            lang: lang.to_owned(),
            default,
            forced,
            channels,
        }
    }

    fn v(id: u32) -> TrackRole {
        role(id, Media::Video, "und", false, false, 0)
    }

    fn a(id: u32, lang: &str, channels: u32, default: bool) -> TrackRole {
        role(id, Media::Audio, lang, default, false, channels)
    }

    fn s(id: u32, lang: &str, forced: bool, default: bool) -> TrackRole {
        role(id, Media::Subtitle, lang, default, forced, 0)
    }

    fn ids(roles: &[TrackRole]) -> Vec<u32> {
        enabled_track_ids(roles).into_iter().collect()
    }

    #[test]
    fn apple_reference_xmen_layout() {
        let roles = [
            v(1),
            a(2, "eng", 2, true),
            a(10, "eng", 6, false),
            a(11, "deu", 2, false),
            a(12, "deu", 6, false),
            s(3, "eng", true, false),
            s(4, "eng", false, false),
            s(5, "deu", true, false),
            s(6, "deu", false, false),
            s(7, "nld", false, false),
        ];
        assert_eq!(ids(&roles), [1, 2, 3]);
    }

    #[test]
    fn home_for_christmas_layout() {
        let roles = [
            v(1),
            a(2, "deu", 6, true),
            a(3, "deu", 2, false),
            a(4, "nor", 6, false),
            a(5, "nor", 2, false),
            s(6, "deu", false, true),
            s(7, "deu", true, true),
            s(8, "eng", false, false),
            s(9, "nor", false, false),
            s(10, "nor", true, true),
        ];
        assert_eq!(ids(&roles), [1, 3, 7]);
    }

    #[test]
    fn surround_only_language_enables_the_surround_track() {
        assert_eq!(
            ids(&[v(1), a(2, "eng", 6, true), a(3, "deu", 2, false)]),
            [1, 2]
        );
    }

    #[test]
    fn no_forced_subtitle_in_audio_language_enables_none() {
        let roles = [
            v(1),
            a(2, "deu", 2, true),
            s(3, "nor", true, false),
            s(4, "deu", false, false),
        ];
        assert_eq!(ids(&roles), [1, 2]);
    }

    #[test]
    fn und_audio_enables_no_subtitle() {
        assert_eq!(
            ids(&[v(1), a(2, "und", 2, true), s(3, "und", true, false)]),
            [1, 2]
        );
    }

    #[test]
    fn no_default_audio_uses_first_audio_language() {
        let roles = [
            v(1),
            a(2, "fra", 6, false),
            a(3, "fra", 2, false),
            a(4, "eng", 2, false),
            s(5, "fra", true, false),
        ];
        assert_eq!(ids(&roles), [1, 3, 5]);
    }

    #[test]
    fn roles_from_probe_maps_hex_ids_and_skips_cover_art() {
        let probe = json!({"streams": [
            {"id": "0x7", "codec_type": "video", "disposition": {"attached_pic": 0}},
            {"id": "0x8", "codec_type": "video", "disposition": {"attached_pic": 1}},
            {"id": "0x9", "codec_type": "audio", "channels": 2, "tags": {"language": "de"},
             "disposition": {"default": 1}},
            {"id": "0xa", "codec_type": "subtitle", "tags": {"language": "deu", "title": "Forced"},
             "disposition": {}},
            {"id": "0xb", "codec_type": "data"},
            {"codec_type": "audio", "channels": 2},
        ]});
        let roles = roles_from_probe(&probe);
        let found: Vec<u32> = roles.iter().map(|r| r.track_id).collect();
        assert_eq!(found, [7, 9, 10]);
        assert_eq!(roles[1], role(9, Media::Audio, "deu", true, false, 2));
        assert!(roles[2].forced && roles[2].media == Media::Subtitle);
        assert_eq!(roles[0].channels, 0);
    }

    #[test]
    fn a_track_id_beyond_u32_is_skipped_instead_of_overflowing() {
        let probe = json!({"streams": [
            {"id": "0xFFFFFFFFFFFFFFFFFF", "codec_type": "video"},
            {"id": "0x100000000", "codec_type": "video"},
            {"id": "0xffffffff", "codec_type": "video"},
        ]});
        let found: Vec<u32> = roles_from_probe(&probe)
            .iter()
            .map(|r| r.track_id)
            .collect();
        assert_eq!(found, [u32::MAX]);
    }

    #[test]
    fn describe_role_names_the_kind_of_track() {
        assert_eq!(describe_role(&v(1)), "video");
        assert_eq!(describe_role(&a(2, "deu", 2, false)), "deu stereo audio");
        assert_eq!(describe_role(&a(3, "deu", 6, false)), "deu 5.1 audio");
        assert_eq!(describe_role(&a(4, "eng", 8, false)), "eng 7.1 audio");
        assert_eq!(describe_role(&a(5, "fra", 3, false)), "fra 3-channel audio");
        assert_eq!(describe_role(&a(6, "und", 0, false)), "und audio");
        assert_eq!(
            describe_role(&s(7, "deu", true, false)),
            "deu forced subtitle"
        );
        assert_eq!(
            describe_role(&s(8, "eng", false, false)),
            "eng full subtitle"
        );
    }
}

//! Stream analysis: what happens to video, audio and subtitle tracks (port of
//! `atvprep/analyzers.py`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::constants::{
    ATV_AV1_OK_PROFILES, COMPAT_AUDIO_COPY, FORCE_TRANSCODE_VIDEO, IMAGE_SUB_CODECS,
    TEXT_SUB_CODECS, TRANSCODE_TO_EAC3, normalize_lang,
};
use crate::models::{
    AudioAction, AudioTrack, SubAction, SubSource, SubTrack, VideoAction, VideoInfo,
};
use crate::probe_helpers::{
    h264_is_atv_compatible, hevc_is_atv_compatible, hevc_sample_tag, is_atmos_audio,
    is_dolby_vision, is_hdr, object, py_int,
};
use crate::sidecars::{
    guess_lang_from_srt_name, lang_from_srt_path, sub_info_from_srt_name, sub_info_from_stream,
    sub_kind,
};

fn text(stream: &Value, key: &str) -> String {
    stream
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// What happens to the first video stream: copy it (with the right tag) or transcode to HEVC.
///
/// `crf` is not used here; the planner forwards it to the libx265 `-x265-params`.
#[must_use]
pub fn analyze_video(stream: &Value, prefer_hevc10: bool, _crf: u32, keep_av1: bool) -> VideoInfo {
    let codec = text(stream, "codec_name").to_lowercase();
    let hdr = is_hdr(stream);
    let dolby_vision = is_dolby_vision(stream);
    let info = |action: VideoAction, tag: Option<&str>, dolby_vision: bool| VideoInfo {
        codec: codec.clone(),
        profile: text(stream, "profile"),
        width: u32::try_from(py_int(stream.get("width"))).unwrap_or(0),
        height: u32::try_from(py_int(stream.get("height"))).unwrap_or(0),
        hdr,
        dolby_vision,
        pix_fmt: text(stream, "pix_fmt").to_lowercase(),
        action,
        tag: tag.map(str::to_owned),
        color_transfer: text(stream, "color_transfer").to_lowercase(),
        color_primaries: text(stream, "color_primaries").to_lowercase(),
    };

    // Dolby Vision: always copy conservatively.
    if dolby_vision {
        let tag = (codec == "hevc").then(|| hevc_sample_tag(stream));
        return info(VideoAction::Copy, tag, true);
    }
    // Codecs that must always be transcoded to HEVC 10-bit.
    if FORCE_TRANSCODE_VIDEO.contains(&codec.as_str()) {
        return info(VideoAction::Transcode, Some("hvc1"), false);
    }
    // AV1: copy only the Main profile (hardware-decoded on Apple TV 4K 3rd gen and later).
    if codec == "av1" {
        let profile = text(stream, "profile").to_lowercase();
        return if keep_av1 && ATV_AV1_OK_PROFILES.contains(&profile.trim()) {
            info(VideoAction::Copy, None, false)
        } else {
            info(VideoAction::Transcode, Some("hvc1"), false)
        };
    }
    // HEVC: copy with the hvc1 tag only for an Apple-TV-compatible profile and pixel format.
    if codec == "hevc" {
        let action = if hevc_is_atv_compatible(stream) {
            VideoAction::Copy
        } else {
            VideoAction::Transcode
        };
        return info(action, Some("hvc1"), false);
    }
    // H.264: transcode if 4K / HDR / prefer_hevc10 / incompatible profile ("High 10" is not
    // hardware-decoded on Apple TV).
    let width = py_int(stream.get("width"));
    let height = py_int(stream.get("height"));
    let needs_transcode =
        hdr || width >= 3840 || height >= 2160 || prefer_hevc10 || !h264_is_atv_compatible(stream);
    if needs_transcode {
        info(VideoAction::Transcode, Some("hvc1"), false)
    } else {
        info(VideoAction::Copy, None, false)
    }
}

const fn codec_rank(codec: &str) -> u8 {
    match codec.as_bytes() {
        b"eac3" => 3,
        b"ac3" => 2,
        b"aac" => 1,
        _ => 0,
    }
}

/// Sort key: higher is preferred.
#[must_use]
pub fn audio_score(track: &AudioTrack) -> (bool, u8, bool, u32, u8, i64) {
    let lang_rank = match track.lang.as_str() {
        "deu" => 2,
        "eng" => 1,
        _ => 0,
    };
    (
        track.is_atmos,
        codec_rank(&track.codec),
        !track.is_transcoded,
        track.channels,
        lang_rank,
        -i64::from(track.index),
    )
}

struct Parsed {
    codec: String,
    channels: u32,
    lang: String,
    index: u32,
    atmos: bool,
}

/// The primary non-German language, or `None` if only German/undetermined streams exist.
fn pick_original_lang(parsed: &[Parsed]) -> Option<String> {
    let mut best: Vec<(&str, (u32, u8, i64))> = Vec::new();
    for p in parsed.iter().filter(|p| p.lang != "deu" && p.lang != "und") {
        let score = (p.channels, codec_rank(&p.codec), -i64::from(p.index));
        match best.iter_mut().find(|(lang, _)| *lang == p.lang) {
            Some((_, current)) if score > *current => *current = score,
            Some(_) => {}
            None => best.push((&p.lang, score)),
        }
    }
    // The first language with the highest score wins, like Python's `max` over a dict.
    let mut winner: Option<&(&str, (u32, u8, i64))> = None;
    for candidate in &best {
        if winner.is_none_or(|w| candidate.1 > w.1) {
            winner = Some(candidate);
        }
    }
    winner.map(|(lang, _)| (*lang).to_owned())
}

#[allow(clippy::too_many_arguments)]
fn track(
    index: u32,
    codec: &str,
    channels: u32,
    lang: &str,
    is_atmos: bool,
    action: AudioAction,
) -> AudioTrack {
    AudioTrack {
        index,
        codec: codec.to_owned(),
        channels,
        lang: lang.to_owned(),
        is_atmos,
        action,
        is_transcoded: action != AudioAction::Copy,
        is_default: false,
    }
}

fn is_pcm(codec: &str) -> bool {
    codec.starts_with("pcm_")
}

fn parse_streams(streams: &[Value], skip: &BTreeSet<i64>) -> Vec<Parsed> {
    streams
        .iter()
        .filter(|s| !skip.contains(&py_int(s.get("index"))))
        .map(|s| Parsed {
            codec: s
                .get("codec_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase(),
            channels: u32::try_from(py_int(s.get("channels"))).unwrap_or(0),
            lang: normalize_lang(
                object(s, "tags")
                    .and_then(|t| t.get("language"))
                    .and_then(Value::as_str),
            ),
            index: u32::try_from(py_int(s.get("index"))).unwrap_or(0),
            atmos: is_atmos_audio(s),
        })
        .collect()
}

/// The streams of the languages worth keeping: German plus the original language.
fn kept_streams(parsed: &[Parsed]) -> Vec<&Parsed> {
    let all_langs: BTreeSet<&str> = parsed.iter().map(|p| p.lang.as_str()).collect();
    let german = all_langs.contains("deu");
    let original = pick_original_lang(parsed);
    let keep: BTreeSet<&str> = match (german, original.as_deref()) {
        (true, Some(orig)) => BTreeSet::from(["deu", orig]),
        (true, None) => BTreeSet::from(["deu"]),
        (false, Some(orig)) => BTreeSet::from([orig]),
        (false, None) => all_langs,
    };
    let kept: Vec<&Parsed> = parsed
        .iter()
        .filter(|p| keep.contains(p.lang.as_str()))
        .collect();
    if kept.is_empty() {
        parsed.iter().collect()
    } else {
        kept
    }
}

/// Split into stereo and multi-channel candidates, and the languages that have a stereo track.
fn split_tracks(kept: &[&Parsed]) -> (Vec<AudioTrack>, Vec<AudioTrack>, BTreeSet<String>) {
    let mut stereo: Vec<AudioTrack> = Vec::new();
    let mut multi: Vec<AudioTrack> = Vec::new();
    let mut stereo_langs: BTreeSet<String> = BTreeSet::new();
    for p in kept {
        let (codec, ch, lang) = (p.codec.as_str(), p.channels, p.lang.as_str());
        let transcodable = TRANSCODE_TO_EAC3.contains(&codec) || is_pcm(codec);
        if ch <= 2 {
            if codec == "aac" || COMPAT_AUDIO_COPY.contains(&codec) {
                stereo.push(track(p.index, codec, ch, lang, false, AudioAction::Copy));
                stereo_langs.insert(lang.to_owned());
            } else if transcodable {
                multi.push(track(
                    p.index,
                    "eac3",
                    ch,
                    lang,
                    false,
                    AudioAction::TranscodeEac3,
                ));
            }
        } else if codec == "aac" {
            multi.push(track(p.index, "aac", ch, lang, false, AudioAction::Copy));
        } else if COMPAT_AUDIO_COPY.contains(&codec) {
            multi.push(track(p.index, codec, ch, lang, p.atmos, AudioAction::Copy));
        } else if transcodable {
            multi.push(track(
                p.index,
                "eac3",
                ch,
                lang,
                false,
                AudioAction::TranscodeEac3,
            ));
        }
    }
    (stereo, multi, stereo_langs)
}

/// An AAC stereo downmix for each multi-channel language without a native stereo track.
fn add_downmixes(
    stereo: &mut Vec<AudioTrack>,
    multi: &[AudioTrack],
    stereo_langs: &mut BTreeSet<String>,
) {
    let mut best: Vec<&AudioTrack> = Vec::new();
    for t in multi {
        match best.iter_mut().find(|current| current.lang == t.lang) {
            Some(current) if audio_score(t) > audio_score(current) => *current = t,
            Some(_) => {}
            None => best.push(t),
        }
    }
    for src in best {
        if !stereo_langs.contains(&src.lang) && src.channels > 2 {
            stereo.push(track(
                src.index,
                "aac",
                2,
                &src.lang,
                false,
                AudioAction::Downmix,
            ));
            stereo_langs.insert(src.lang.clone());
        }
    }
}

/// The ordered audio tracks of the output MP4.
///
/// Order: language (deu, eng, others alphabetically); within a language the surround track comes
/// before its stereo track. Only the first track is the default. Every language keeps a stereo
/// track: a native one, or an AAC 2.0 downmix of its best surround track. Multi-channel tracks are
/// both copied (surround) AND downmixed (stereo).
///
/// Language selection: German plus the best non-German language (the original), or whichever of
/// the two exists, or every language when all are undetermined. Output cap: at most 2 stereo and
/// 2 multi-channel tracks.
#[must_use]
pub fn analyze_audio(streams: &[Value], skip: &BTreeSet<i64>) -> Vec<AudioTrack> {
    let parsed = parse_streams(streams, skip);
    if parsed.is_empty() {
        return Vec::new();
    }
    let (mut stereo, mut multi, mut stereo_langs) = split_tracks(&kept_streams(&parsed));
    add_downmixes(&mut stereo, &multi, &mut stereo_langs);

    stereo.sort_by_key(|t| std::cmp::Reverse(audio_score(t)));
    stereo.truncate(2);
    multi.sort_by_key(|t| std::cmp::Reverse(audio_score(t)));
    multi.truncate(2);

    // Guarantee at least one stereo track (a downmix from the best multi-channel track).
    if stereo.is_empty() {
        let Some(src) = multi.first() else {
            return Vec::new();
        };
        stereo.push(track(
            src.index,
            "aac",
            2,
            &src.lang,
            false,
            AudioAction::Downmix,
        ));
    }

    let mut result = stereo;
    result.append(&mut multi);
    // deu, then eng, then the others alphabetically; within a language multichannel first.
    result.sort_by_key(|t| {
        let rank = match t.lang.as_str() {
            "deu" => 0_u8,
            "eng" => 1,
            _ => 2,
        };
        (rank, t.lang.clone(), u8::from(t.channels <= 2))
    });
    for (position, t) in result.iter_mut().enumerate() {
        t.is_default = position == 0;
    }
    result
}

/// The ordered subtitle tracks of the output MP4.
///
/// - Image subtitles (PGS/VobSub) are listed with the action `skip`.
/// - Language filter: German, English and the audio languages.
/// - An internal track wins over an external one of the same language and kind (normal, forced or
///   SDH); a forced-only track inside the file does not stop a full subtitle from joining.
/// - Of several normal tracks of one language only the first is kept; forced and SDH tracks always.
/// - Sort: language (deu, eng, others alphabetically), then normal, SDH, forced; only the first
///   active track is the default.
#[must_use]
pub fn analyze_subs(
    streams: &[Value],
    sidecar_srts: &[PathBuf],
    audio_langs: &BTreeSet<String>,
    video: Option<&Path>,
) -> Vec<SubTrack> {
    let wanted = |lang: &str| lang == "deu" || lang == "eng" || audio_langs.contains(lang);
    let mut internal_kinds: BTreeSet<(String, &'static str)> = BTreeSet::new();
    let mut candidates: Vec<SubTrack> = Vec::new();

    for stream in streams {
        let codec = text(stream, "codec_name").to_lowercase();
        let language = object(stream, "tags")
            .and_then(|t| t.get("language"))
            .and_then(Value::as_str);
        let lang = normalize_lang(language);
        let stream_idx = u32::try_from(py_int(stream.get("index"))).unwrap_or(0);
        let internal = |forced: bool, sdh: bool, action: SubAction| SubTrack {
            input_idx: 0,
            stream_idx,
            codec: codec.clone(),
            lang: lang.clone(),
            forced,
            sdh,
            source: SubSource::Internal,
            action,
            is_default: false,
        };
        if IMAGE_SUB_CODECS.contains(&codec.as_str()) {
            candidates.push(internal(false, false, SubAction::Skip));
        } else if TEXT_SUB_CODECS.contains(&codec.as_str()) && wanted(&lang) {
            let (forced, sdh) = sub_info_from_stream(stream);
            internal_kinds.insert((lang.clone(), sub_kind(forced, sdh)));
            candidates.push(internal(forced, sdh, SubAction::MovText));
        }
    }

    for (i, srt) in sidecar_srts.iter().enumerate() {
        let lang = video.map_or_else(
            || lang_from_srt_path(srt),
            |v| guess_lang_from_srt_name(v, srt),
        );
        if !wanted(&lang) {
            continue;
        }
        let (forced, sdh) = sub_info_from_srt_name(srt);
        // Internal wins: skip this sidecar if the file already has that language in that kind.
        if internal_kinds.contains(&(lang.clone(), sub_kind(forced, sdh))) {
            continue;
        }
        candidates.push(SubTrack {
            input_idx: u32::try_from(i + 1).unwrap_or(u32::MAX), // input 0 is the video
            stream_idx: 0,                                       // each SRT file has one stream
            codec: "srt".to_owned(),
            lang,
            forced,
            sdh,
            source: SubSource::External,
            action: SubAction::MovText,
            is_default: false,
        });
    }

    // For each language keep only the first normal track; skipped, forced and SDH always stay.
    let mut seen_normal: BTreeSet<String> = BTreeSet::new();
    candidates.retain(|t| {
        t.action == SubAction::Skip || t.forced || t.sdh || seen_normal.insert(t.lang.clone())
    });

    candidates.sort_by_key(|t| {
        let lang_order = match t.lang.as_str() {
            "deu" => 0_u8,
            "eng" => 1,
            _ => 2,
        };
        let type_order = if t.forced { 2_u8 } else { u8::from(t.sdh) };
        (lang_order, t.lang.clone(), type_order, t.stream_idx)
    });

    if let Some(first) = candidates.iter_mut().find(|t| t.action != SubAction::Skip) {
        first.is_default = true;
    }
    candidates
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::Value;

    use super::*;

    #[derive(Deserialize)]
    struct VideoCase {
        stream: Value,
        prefer_hevc10: bool,
        crf: u32,
        keep_av1: bool,
    }

    #[test]
    fn analyze_video_matches_the_python_oracle() {
        let cases = load_golden::<VideoCase, Value>("analyze_video");
        assert!(cases.len() > 100);
        for case in cases {
            let i = &case.input;
            let info = analyze_video(&i.stream, i.prefer_hevc10, i.crf, i.keep_av1);
            assert_eq!(
                serde_json::to_value(&info).unwrap(),
                case.expected,
                "{}",
                case.name
            );
        }
    }

    #[derive(Deserialize)]
    struct AudioCase {
        kind: String,
        #[serde(default)]
        streams: Vec<Value>,
        #[serde(default)]
        skip: BTreeSet<i64>,
        track: Option<AudioTrack>,
    }

    #[test]
    fn analyze_audio_and_score_match_the_python_oracle() {
        let cases = load_golden::<AudioCase, Value>("analyze_audio");
        assert!(cases.len() >= 40);
        for case in cases {
            let input = &case.input;
            let actual = if input.kind == "score" {
                let score = audio_score(input.track.as_ref().unwrap());
                serde_json::json!([score.0, score.1, score.2, score.3, score.4, score.5])
            } else {
                serde_json::to_value(analyze_audio(&input.streams, &input.skip)).unwrap()
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn only_the_first_track_is_default() {
        let streams: Vec<Value> = vec![
            serde_json::json!({"index": 1, "codec_name": "eac3", "channels": 6, "tags": {"language": "deu"}}),
            serde_json::json!({"index": 2, "codec_name": "eac3", "channels": 6, "tags": {"language": "eng"}}),
        ];
        let tracks = analyze_audio(&streams, &BTreeSet::new());
        assert_eq!(tracks.iter().filter(|t| t.is_default).count(), 1);
        assert!(tracks[0].is_default);
    }

    #[derive(Deserialize)]
    struct SubsCase {
        streams: Vec<Value>,
        sidecars: Vec<String>,
        audio_langs: BTreeSet<String>,
        video: Option<String>,
    }

    #[test]
    fn analyze_subs_matches_the_python_oracle() {
        let cases = load_golden::<SubsCase, Value>("analyze_subs");
        assert!(cases.len() >= 25);
        for case in cases {
            let input = &case.input;
            let sidecars: Vec<PathBuf> = input.sidecars.iter().map(PathBuf::from).collect();
            let video = input.video.as_deref().map(Path::new);
            let tracks = analyze_subs(&input.streams, &sidecars, &input.audio_langs, video);
            assert_eq!(
                serde_json::to_value(tracks).unwrap(),
                case.expected,
                "{}",
                case.name
            );
        }
    }
}

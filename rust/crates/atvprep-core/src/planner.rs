//! The conversion plan: analyse the streams and build the ffmpeg command (port of `build_plan` in
//! `atvprep/planner.py`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::analyzers::{analyze_audio, analyze_subs, analyze_video};
use crate::compliance::atoms::is_fragmented;
use crate::constants::{IMAGE_SUB_CODECS, handler_name};
use crate::ffprobe::Ffprobe;
use crate::ffprobe::Prober;
use crate::models::{AudioAction, AudioTrack, Plan, SubAction, VideoAction, VideoInfo};
use crate::mp4::patch::is_faststart;
use crate::postfix::read_ftyp;
use crate::probe_helpers::{
    ProbeContext, dolby_vision_profile, h264_is_atv_compatible, hevc_is_atv_compatible,
    is_atmos_audio, is_hdr, py_int, truthy,
};
use crate::pyfmt::{repr_str, str_of};
use crate::sidecars::classify_sidecars;

/// Length-prefixed samples to Annex B (the form `extract_extradata` reads), then the parameter
/// sets found in the first packets become the stream's extradata, which the MP4 muxer writes as a
/// complete `hvcC`.
pub const HEVC_PARAMETER_SET_BSF: &str = "hevc_mp4toannexb,extract_extradata";

/// Why no plan could be made.
#[derive(Debug)]
pub enum PlanError {
    /// The source has no video stream.
    NoVideo,
    /// A path cannot be written into the argv without changing it.
    NonUtf8Path(PathBuf),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoVideo => f.write_str("Keine Videospur gefunden."),
            Self::NonUtf8Path(path) => write!(f, "path is not valid UTF-8: {}", path.display()),
        }
    }
}

impl std::error::Error for PlanError {}

/// The knobs of `build_plan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanOptions {
    pub prefer_hevc10: bool,
    pub crf: u32,
    pub keep_av1: bool,
    /// Opt-in DTS normalisation (`-fflags +genpts`, `-avoid_negative_ts make_zero`): `make_zero`
    /// can perturb E-AC-3 priming-sample edit lists on well-formed files, so it never runs on its own.
    pub fix_dts: bool,
    /// Audio stream indices to leave out (duplicates of an earlier track).
    pub skip_audio: BTreeSet<i64>,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            prefer_hevc10: false,
            crf: 18,
            keep_av1: false,
            fix_dts: false,
            skip_audio: BTreeSet::new(),
        }
    }
}

/// Languages (sorted) for which the planner adds a stereo downmix.
#[must_use]
pub fn stereo_gaps(planned: &[AudioTrack]) -> Vec<String> {
    planned
        .iter()
        .filter(|t| t.action == AudioAction::Downmix)
        .map(|t| t.lang.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The video track language: the original (non-dubbed) one, so a German dub of an English film is
/// tagged `eng`, not `deu` (reverse audio priority: others, then eng, then deu).
#[must_use]
pub fn video_lang(audio: &[AudioTrack]) -> String {
    audio
        .iter()
        .map(|t| t.lang.as_str())
        .filter(|lang| *lang != "und")
        .max_by_key(|lang| {
            let rank = match *lang {
                "deu" => 0_u8,
                "eng" => 1,
                _ => 2,
            };
            (rank, (*lang).to_owned())
        })
        .unwrap_or("und")
        .to_owned()
}

fn path_str(path: &Path) -> Result<&str, PlanError> {
    path.to_str()
        .ok_or_else(|| PlanError::NonUtf8Path(path.to_path_buf()))
}

fn codec_of(stream: &Value) -> String {
    stream
        .get("codec_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase()
}

/// `float(format["duration"])`, or `None` when it is missing or not a number.
fn duration_of(probe: &Value) -> Option<f64> {
    match probe.get("format")?.get("duration")? {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn video_warnings(info: &VideoInfo, stream: &Value, keep_av1: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    let transcode = info.action == VideoAction::Transcode;
    if keep_av1 && info.codec == "av1" && info.action == VideoAction::Copy {
        warnings.push(
            "AV1-Video wird per copy gemuxt (--keep-av1). \
             Erfordert Apple TV 4K 3rd gen + aktualisiertes Wave DeviceProfile."
                .to_owned(),
        );
    }
    if transcode && info.codec == "h264" && !h264_is_atv_compatible(stream) {
        let shown = [&info.profile, &info.pix_fmt]
            .into_iter()
            .find(|s| !s.is_empty());
        warnings.push(format!(
            "H.264 Profil '{}' nicht ATV-hardware-kompatibel → Transcode zu HEVC 10-bit.",
            shown.map_or("unbekannt", String::as_str)
        ));
    }
    if transcode
        && info.codec == "hevc"
        && !info.dolby_vision
        && !is_hdr(stream)
        && !hevc_is_atv_compatible(stream)
    {
        let profile = if info.profile.is_empty() {
            "unbekannt"
        } else {
            &info.profile
        };
        warnings.push(format!(
            "HEVC Profil '{profile}' nicht ATV-hardware-kompatibel → Transcode zu HEVC 10-bit."
        ));
    }
    if info.codec == "av1" && transcode && keep_av1 {
        warnings.push(format!(
            "AV1 Profil '{}' nicht ATV-kompatibel (nur Main-Profil) → Transcode zu HEVC 10-bit.",
            info.profile
        ));
    }
    if info.dolby_vision && info.codec != "hevc" {
        warnings.push(format!(
            "Dolby Vision erkannt, aber Codec ist nicht HEVC ({}). Bitte Quelle prüfen.",
            info.codec
        ));
    }
    if info.dolby_vision {
        warnings.push(match dolby_vision_profile(stream) {
            Some(7) => "Dolby Vision Profile 7 (BL+EL) erkannt. Enhancement Layer geht im \
                        MP4-Container verloren. Für verlustfreie DV-P7-Ausgabe MKV-Container verwenden."
                .to_owned(),
            Some(n) => format!("Dolby Vision Profile {n} erkannt — Copy-Mux in MP4."),
            None => "Dolby Vision erkannt (Profil unbekannt) — Copy-Mux in MP4.".to_owned(),
        });
    }
    if info.action == VideoAction::Copy && info.codec != "hevc" && info.codec != "h264" {
        warnings.push(format!(
            "Video-Codec '{}' wird per copy gemuxt — MP4 könnte Probleme verursachen.",
            info.codec
        ));
    }
    warnings
}

fn subtitle_warnings(sub_streams: &[Value]) -> Vec<String> {
    let mut warnings = Vec::new();
    let image = sub_streams
        .iter()
        .filter(|s| IMAGE_SUB_CODECS.contains(&codec_of(s).as_str()))
        .count();
    if image > 0 {
        warnings.push(format!(
            "Bild-Untertitel (PGS/VobSub) übersprungen: {image} Spur(en) sind nicht MP4-kompatibel."
        ));
    }
    let webvtt = sub_streams
        .iter()
        .filter(|s| codec_of(s) == "webvtt")
        .count();
    if webvtt > 0 {
        warnings.push(format!(
            "WebVTT-Untertitel ({webvtt} Spur(en)): erweiterte Formatierungsmerkmale gehen bei \
             Konvertierung zu mov_text verloren."
        ));
    }
    warnings
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

fn video_args(info: &VideoInfo, crf: u32, infile: &Path, prober: &dyn Prober) -> Vec<String> {
    let mut cmd = strings(&["-map", "0:v:0"]);
    if info.action == VideoAction::Copy {
        cmd.extend(strings(&["-c:v", "copy"]));
        if let Some(tag) = &info.tag {
            cmd.extend(["-tag:v".to_owned(), tag.clone()]);
        }
        if info.codec == "hevc" && prober.hevc_has_no_parameter_sets(infile) {
            cmd.extend(["-bsf:v".to_owned(), HEVC_PARAMETER_SET_BSF.to_owned()]);
        }
        return cmd;
    }
    let mut x265_params = format!("crf={crf}");
    let mut hdr_flags: Vec<String> = Vec::new();
    if info.hdr {
        let transfer = match info.color_transfer.as_str() {
            t @ ("smpte2084" | "arib-std-b67") => t,
            _ => "smpte2084",
        };
        let primaries = if info.color_primaries.is_empty() {
            "bt2020"
        } else {
            &info.color_primaries
        };
        x265_params = format!(
            "crf={crf}:colorprim={primaries}:transfer={transfer}\
             :colormatrix=bt2020nc:hdr-opt=1:repeat-headers=1"
        );
        hdr_flags = strings(&[
            "-color_primaries",
            primaries,
            "-color_trc",
            transfer,
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
        ]);
    }
    cmd.extend(strings(&[
        "-c:v",
        "libx265",
        "-pix_fmt",
        "yuv420p10le",
        "-x265-params",
    ]));
    cmd.push(x265_params);
    cmd.extend(strings(&["-tag:v", "hvc1"]));
    cmd.extend(hdr_flags);
    cmd
}

fn audio_args(tracks: &[AudioTrack], audio_streams: &[Value]) -> Vec<String> {
    // Global stream index -> position among the audio streams; a later duplicate wins.
    let relative: BTreeMap<i64, usize> = audio_streams
        .iter()
        .enumerate()
        .map(|(i, s)| (py_int(s.get("index")), i))
        .collect();
    let mut cmd = Vec::new();
    for (i, track) in tracks.iter().enumerate() {
        let rel = relative.get(&i64::from(track.index)).copied().unwrap_or(0);
        cmd.extend(["-map".to_owned(), format!("0:a:{rel}")]);
        match track.action {
            AudioAction::Copy => cmd.extend([format!("-c:a:{i}"), "copy".to_owned()]),
            AudioAction::Downmix => cmd.extend([
                format!("-c:a:{i}"),
                "aac".to_owned(),
                format!("-b:a:{i}"),
                "192k".to_owned(),
                format!("-ac:a:{i}"),
                "2".to_owned(),
            ]),
            AudioAction::TranscodeEac3 => cmd.extend([
                format!("-c:a:{i}"),
                "eac3".to_owned(),
                format!("-b:a:{i}"),
                "768k".to_owned(),
            ]),
        }
        cmd.extend([
            format!("-metadata:s:a:{i}"),
            format!("language={}", track.lang),
        ]);
        cmd.extend([
            format!("-metadata:s:a:{i}"),
            format!("handler_name={}", handler_name(&track.lang)),
        ]);
        cmd.extend([
            format!("-disposition:a:{i}"),
            (if track.is_default { "default" } else { "0" }).to_owned(),
        ]);
    }
    cmd
}

fn subtitle_args(tracks: &[crate::models::SubTrack]) -> Vec<String> {
    let mut cmd = Vec::new();
    for (i, sub) in tracks
        .iter()
        .filter(|t| t.action != SubAction::Skip)
        .enumerate()
    {
        cmd.extend([
            "-map".to_owned(),
            format!("{}:{}", sub.input_idx, sub.stream_idx),
        ]);
        cmd.extend([format!("-c:s:{i}"), "mov_text".to_owned()]);
        cmd.extend([
            format!("-metadata:s:s:{i}"),
            format!("language={}", sub.lang),
        ]);
        cmd.extend([
            format!("-metadata:s:s:{i}"),
            format!("handler_name={}", handler_name(&sub.lang)),
        ]);
        let mut title = Vec::new();
        if sub.forced {
            title.push("Forced".to_owned());
        }
        title.push(sub.lang.to_uppercase());
        if sub.sdh {
            title.push("(SDH)".to_owned());
        }
        cmd.extend([
            format!("-metadata:s:s:{i}"),
            format!("title={}", title.join(" ")),
        ]);
        let disposition = if sub.forced {
            "forced+default"
        } else if sub.sdh {
            "hearing_impaired"
        } else if sub.is_default {
            "default"
        } else {
            "0"
        };
        cmd.extend([format!("-disposition:s:{i}"), disposition.to_owned()]);
    }
    cmd
}

/// Build the full conversion plan: analyse the streams and construct the ffmpeg command.
///
/// `ctx` carries the probe results cached for this file; `prober` is only asked when an HEVC
/// stream is copied.
///
/// # Errors
///
/// [`PlanError::NoVideo`] without a video stream; [`PlanError::NonUtf8Path`] when a path cannot be
/// written into the argv unchanged.
pub fn build_plan(
    infile: &Path,
    probe: &Value,
    out_path: &Path,
    sidecars: &[PathBuf],
    options: &PlanOptions,
    ctx: &ProbeContext,
    prober: &dyn Prober,
) -> Result<Plan, PlanError> {
    let infile_arg = path_str(infile)?;
    let out_arg = path_str(out_path)?;
    let sidecar_args = sidecars
        .iter()
        .map(|s| path_str(s))
        .collect::<Result<Vec<_>, _>>()?;

    let needs_dts_fix = options.fix_dts && ctx.first_video_dts.is_some_and(|dts| dts < 0);
    let streams = probe
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let of_kind = |kind: &str| -> Vec<Value> {
        streams
            .iter()
            .filter(|s| s.get("codec_type").and_then(Value::as_str) == Some(kind))
            .cloned()
            .collect()
    };
    let (video_streams, audio_streams, sub_streams) =
        (of_kind("video"), of_kind("audio"), of_kind("subtitle"));
    let duration_s = duration_of(probe);
    let first_video = video_streams.first().ok_or(PlanError::NoVideo)?;

    let video_info = analyze_video(
        first_video,
        options.prefer_hevc10,
        options.crf,
        options.keep_av1,
    );
    let mut warnings = video_warnings(&video_info, first_video, options.keep_av1);
    warnings.extend(subtitle_warnings(&sub_streams));

    let audio_tracks = analyze_audio(&audio_streams, &options.skip_audio);
    if audio_tracks.is_empty() {
        warnings
            .push("Keine kompatible Audiospur gefunden — MP4 wird ohne Audio erstellt.".to_owned());
    }
    let audio_langs: BTreeSet<String> = audio_tracks
        .iter()
        .filter(|t| t.lang != "und")
        .map(|t| t.lang.clone())
        .collect();
    let sub_tracks = analyze_subs(&sub_streams, sidecars, &audio_langs, Some(infile));

    // -fflags +genpts regenerates missing PTS from DTS and -avoid_negative_ts make_zero shifts
    // every stream so the first DTS is 0; together they remove the negative-DTS edit lists that
    // AVPlayer on tvOS handles poorly. Opt-in through `fix_dts`.
    let mut cmd = strings(&["ffmpeg", "-hide_banner"]);
    if needs_dts_fix {
        cmd.extend(strings(&["-fflags", "+genpts"]));
    }
    cmd.extend(strings(&["-y", "-i", infile_arg]));
    for srt in &sidecar_args {
        cmd.extend(strings(&["-i", srt]));
    }
    cmd.extend(video_args(&video_info, options.crf, infile, prober));
    cmd.extend([
        "-metadata:s:v:0".to_owned(),
        format!("language={}", video_lang(&audio_tracks)),
    ]);
    cmd.extend(audio_args(&audio_tracks, &audio_streams));
    cmd.extend(subtitle_args(&sub_tracks));

    // No +faststart: ffmpeg would rewrite the whole file once more at the end. The moov patch
    // moves the moov to the front in the one copy it makes anyway.
    cmd.extend(strings(&["-map_chapters", "-1"]));
    if needs_dts_fix {
        cmd.extend(strings(&["-avoid_negative_ts", "make_zero"]));
    }
    cmd.extend(strings(&["-strict", "unofficial", out_arg]));

    Ok(Plan {
        video_info: Some(video_info),
        audio_tracks,
        sub_tracks,
        ffmpeg_cmd: cmd,
        warnings,
        in_path: infile.to_path_buf(),
        out_path: out_path.to_path_buf(),
        duration_s,
    })
}

/// Extensions that could plausibly carry a faststart mp4; anything else must be remuxed.
const MP4_LIKE_EXTENSIONS: [&str; 3] = [".mp4", ".m4v", ".mov"];

/// The switches of [`detect_issues`].
#[derive(Debug, Clone, Copy)]
pub struct DetectOptions {
    /// Report a negative initial video DTS (otherwise the remux would not normalise it anyway).
    pub fix_dts: bool,
    /// Report audio without a language tag (a remux writes `und` again, so the escalation skips it).
    pub untagged_audio: bool,
}

fn language_of(stream: &Value) -> String {
    stream
        .get("tags")
        .and_then(|t| t.get("language"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase()
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Container and layout reasons (the file itself, not its streams).
fn layout_reasons(
    infile: &Path,
    probe: &Value,
    ctx: Option<&ProbeContext>,
    ffprobe: &Ffprobe,
    options: DetectOptions,
) -> Vec<String> {
    let mut reasons = Vec::new();
    let container = probe
        .get("format")
        .and_then(|f| f.get("format_name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    if !container.contains("mp4") && !container.contains("mov") {
        reasons.push(format!("container={} (not mp4/mov)", repr_str(&container)));
    }
    let dts = if let Some(ctx) = ctx {
        if !ctx.is_faststart {
            reasons.push("moov after mdat (missing faststart)".to_owned());
        }
        ctx.first_video_dts
    } else {
        // An unreadable file is left for ffmpeg to report.
        if is_faststart(infile).is_ok_and(|fast| !fast) {
            reasons.push("moov after mdat (missing faststart)".to_owned());
        }
        ffprobe.first_video_dts(infile)
    };
    if let (true, Some(dts)) = (options.fix_dts, dts)
        && dts < 0
    {
        reasons.push(format!("negative initial video DTS ({dts})"));
    }
    // Blockers only a remux can fix: postfix cannot defragment or add dby1.
    if is_fragmented(infile).unwrap_or(false) {
        reasons.push("fragmented mp4 (mvex in moov)".to_owned());
    }
    if streams_of_kind(probe, "audio").iter().any(is_atmos_audio)
        && let Ok(ftyp) = read_ftyp(infile)
        && !ftyp.compatible_brands.iter().any(|b| b == "dby1")
    {
        reasons.push("Atmos audio but ftyp lacks dby1".to_owned());
    }
    reasons
}

fn streams_of_kind(probe: &Value, kind: &str) -> Vec<Value> {
    let streams = probe
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    streams
        .iter()
        .filter(|s| s.get("codec_type").and_then(Value::as_str) == Some(kind))
        .cloned()
        .collect()
}

/// Audio reasons: the planner keeps every stream, each language has a stereo track, no duplicates.
fn audio_reasons(probe: &Value, duplicates: &BTreeSet<i64>, untagged_audio: bool) -> Vec<String> {
    let audio_streams = streams_of_kind(probe, "audio");
    let planned = analyze_audio(&audio_streams, duplicates);
    let kept = planned
        .iter()
        .filter(|t| t.action != AudioAction::Downmix)
        .count();
    let expected = audio_streams.len().cast_signed() - duplicates.len().cast_signed();
    let mut reasons = Vec::new();
    if kept.cast_signed() != expected {
        reasons.push(format!(
            "{} audio stream(s); planner would keep {kept}",
            audio_streams.len()
        ));
    }
    reasons.extend(
        stereo_gaps(&planned)
            .into_iter()
            .map(|lang| format!("no stereo track for {lang}")),
    );
    reasons.extend(
        duplicates
            .iter()
            .map(|i| format!("audio stream #{i} duplicates an earlier stream")),
    );
    let source_codec: BTreeMap<i64, String> = audio_streams
        .iter()
        .map(|s| {
            (
                py_int(s.get("index")),
                s.get("codec_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase(),
            )
        })
        .collect();
    for track in planned
        .iter()
        .filter(|t| t.action == AudioAction::TranscodeEac3)
    {
        let codec = source_codec
            .get(&i64::from(track.index))
            .unwrap_or(&track.codec);
        reasons.push(format!(
            "audio stream #{} codec={codec} needs E-AC-3",
            track.index
        ));
    }
    if untagged_audio
        && let Some(stream) = audio_streams
            .iter()
            .find(|s| ["", "und"].contains(&language_of(s).as_str()))
    {
        reasons.push(format!(
            "audio stream #{} has no language tag",
            str_of(stream.get("index"))
        ));
    }
    let has_default = audio_streams
        .iter()
        .any(|s| truthy(s.get("disposition").and_then(|d| d.get("default"))));
    if !audio_streams.is_empty() && !has_default {
        reasons.push("no audio stream marked default".to_owned());
    }
    reasons
}

/// Subtitle reasons: all must be `mov_text` with a language, no image subtitles.
fn subtitle_reasons(probe: &Value) -> Vec<String> {
    for stream in &streams_of_kind(probe, "subtitle") {
        let codec = stream
            .get("codec_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let index = str_of(stream.get("index"));
        if IMAGE_SUB_CODECS.contains(&codec.as_str()) {
            return vec![format!("image subtitle present (codec={codec})")];
        }
        if !codec.is_empty() && codec != "mov_text" {
            return vec![format!(
                "subtitle stream #{index} codec={codec} (not mov_text)"
            )];
        }
        if ["", "und"].contains(&language_of(stream).as_str()) {
            return vec![format!("subtitle stream #{index} has no language tag")];
        }
    }
    Vec::new()
}

/// Human-readable reasons `infile` needs a rebuild; empty means it already matches what a rebuild
/// would produce and is safe to skip.
///
/// With `ctx` its cached `is_faststart` and DTS are used; without it the file is read here. Checked:
/// the container is mp4/mov, the moov precedes the mdat, the file is not fragmented, Atmos audio
/// implies `dby1`, the first video DTS is not negative (only with `fix_dts`), the planner keeps
/// every audio stream, every language has a stereo track, no stream duplicates an earlier one, no
/// codec needs an E-AC-3 transcode, audio has a language and a default, subtitles are `mov_text`
/// with a language, and no sidecar SRT is still to be merged.
#[must_use]
pub fn detect_issues(
    infile: &Path,
    probe: &Value,
    sidecars: &[PathBuf],
    ctx: Option<&ProbeContext>,
    ffprobe: &Ffprobe,
    duplicates: &BTreeSet<i64>,
    options: DetectOptions,
) -> Vec<String> {
    let extension = infile.extension().map_or_else(String::new, |e| {
        format!(".{}", e.to_string_lossy().to_lowercase())
    });
    if !MP4_LIKE_EXTENSIONS.contains(&extension.as_str()) {
        // No point checking the mp4 structure of another container.
        return vec![format!("source is {extension} (must remux to mp4)")];
    }
    let mut reasons = layout_reasons(infile, probe, ctx, ffprobe, options);
    reasons.extend(audio_reasons(probe, duplicates, options.untagged_audio));
    reasons.extend(subtitle_reasons(probe));

    // Sidecars: only those a remux would still merge, decided exactly as the remux decides it.
    if !sidecars.is_empty() {
        let names: Vec<String> = sidecars.iter().map(|p| file_name_of(p)).collect();
        let streams = probe
            .get("streams")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let pending = classify_sidecars(&file_name_of(infile), &names, Some(streams))
            .iter()
            .filter(|(_, _, state)| state == "pending")
            .count();
        if pending > 0 {
            reasons.push(format!("{pending} sidecar SRT(s) not yet merged"));
        }
    }
    reasons
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Deserialize)]
    struct Options {
        prefer_hevc10: bool,
        crf: u32,
        keep_av1: bool,
        fix_dts: bool,
        skip_audio: BTreeSet<i64>,
    }

    #[derive(Deserialize)]
    struct Context {
        first_video_dts: Option<i64>,
        is_faststart: bool,
    }

    #[derive(Deserialize)]
    struct Input {
        probe: Value,
        sidecars: Vec<String>,
        options: Options,
        ctx: Context,
        no_param_sets: bool,
    }

    #[derive(Deserialize)]
    struct Expected {
        error: Option<String>,
        plan: Option<Value>,
    }

    struct Fake(bool);

    impl Prober for Fake {
        fn hevc_has_no_parameter_sets(&self, _infile: &Path) -> bool {
            self.0
        }
    }

    fn context(probe: &Value, ctx: &Context) -> ProbeContext {
        ProbeContext {
            infile: PathBuf::from("/media/Film.mkv"),
            ffprobe: probe.clone(),
            first_video_dts: ctx.first_video_dts,
            is_faststart: ctx.is_faststart,
        }
    }

    fn options(o: &Options) -> PlanOptions {
        PlanOptions {
            prefer_hevc10: o.prefer_hevc10,
            crf: o.crf,
            keep_av1: o.keep_av1,
            fix_dts: o.fix_dts,
            skip_audio: o.skip_audio.clone(),
        }
    }

    fn plan_json(plan: &Plan) -> Value {
        json!({
            "video_info": plan.video_info,
            "audio_tracks": plan.audio_tracks,
            "sub_tracks": plan.sub_tracks,
            "ffmpeg_cmd": plan.ffmpeg_cmd,
            "warnings": plan.warnings,
            "in_path": plan.in_path.to_string_lossy(),
            "out_path": plan.out_path.to_string_lossy(),
            "duration_s": plan.duration_s,
        })
    }

    #[test]
    fn build_plan_matches_the_python_oracle() {
        let cases = load_golden::<Input, Expected>("build_plan");
        assert!(cases.len() >= 47);
        for case in cases {
            let input = &case.input;
            let sidecars: Vec<PathBuf> = input
                .sidecars
                .iter()
                .map(|n| Path::new("/media").join(n))
                .collect();
            let result = build_plan(
                Path::new("/media/Film.mkv"),
                &input.probe,
                Path::new("/media/Film.mp4"),
                &sidecars,
                &options(&input.options),
                &context(&input.probe, &input.ctx),
                &Fake(input.no_param_sets),
            );
            match (result, &case.expected) {
                (
                    Ok(plan),
                    Expected {
                        error: None,
                        plan: Some(want),
                    },
                ) => {
                    assert_eq!(&plan_json(&plan), want, "{}", case.name);
                }
                (
                    Err(error),
                    Expected {
                        error: Some(want),
                        plan: None,
                    },
                ) => {
                    assert_eq!(&error.to_string(), want, "{}", case.name);
                }
                (other, _) => panic!("{}: unexpected result {other:?}", case.name),
            }
        }
    }

    #[test]
    fn a_source_without_video_is_an_error_with_the_python_text() {
        let probe = json!({"streams": []});
        let ctx = Context {
            first_video_dts: None,
            is_faststart: true,
        };
        let error = build_plan(
            Path::new("/m/a.mkv"),
            &probe,
            Path::new("/m/a.mp4"),
            &[],
            &PlanOptions::default(),
            &context(&probe, &ctx),
            &Fake(false),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Keine Videospur gefunden.");
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_is_refused_instead_of_mangled() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let probe = json!({"streams": [{"index": 0, "codec_type": "video", "codec_name": "h264"}]});
        let ctx = Context {
            first_video_dts: None,
            is_faststart: true,
        };
        let bad = Path::new(OsStr::from_bytes(b"/m/Film\xff.mkv"));
        let attempt = |infile: &Path, out: &Path, sidecars: &[PathBuf]| {
            build_plan(
                infile,
                &probe,
                out,
                sidecars,
                &PlanOptions::default(),
                &context(&probe, &ctx),
                &Fake(false),
            )
        };
        assert!(matches!(
            attempt(bad, Path::new("/m/a.mp4"), &[]),
            Err(PlanError::NonUtf8Path(_))
        ));
        assert!(matches!(
            attempt(Path::new("/m/a.mkv"), bad, &[]),
            Err(PlanError::NonUtf8Path(_))
        ));
        assert!(matches!(
            attempt(
                Path::new("/m/a.mkv"),
                Path::new("/m/a.mp4"),
                &[bad.to_path_buf()]
            ),
            Err(PlanError::NonUtf8Path(_))
        ));
    }

    #[test]
    fn the_video_language_is_the_original_not_the_dub() {
        let track = |lang: &str| crate::models::AudioTrack {
            index: 1,
            codec: "aac".to_owned(),
            channels: 2,
            lang: lang.to_owned(),
            is_atmos: false,
            action: crate::models::AudioAction::Copy,
            is_transcoded: false,
            is_default: false,
        };
        assert_eq!(video_lang(&[track("deu"), track("eng")]), "eng");
        assert_eq!(
            video_lang(&[track("deu"), track("fra"), track("eng")]),
            "fra"
        );
        assert_eq!(video_lang(&[track("und")]), "und");
        assert_eq!(video_lang(&[]), "und");
    }
}

//! The compliance rules: findings that stop a file (blockers) and findings that only show up in
//! the verify output (warnings).
//!
//! Each rule is self-contained and reads what it needs through an [`Audit`]: the text of `mp4info`
//! and `mp4dump`, the ffprobe JSON, and the atom structure of the file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use super::atoms::{AtomError, Top, first_moov_vs_mdat, is_fragmented};
use super::parse::{
    ParseError, flags_enabled, hevc_level_exceeds_52, parse_audio_codec_infos, parse_ftyp_brands,
    parse_mp4info_tracks, parse_track_handlers, parse_video_codec_infos,
};
use super::tools::{Audit, ToolError};
use super::{Severity, Violation};
use crate::escalation::{stereo_twin_gaps, video_reencode_reason};
use crate::mp4::error::Mp4Error;
use crate::mp4::patch::{TagCheck, chapter_track_ids, check_accessibility};
use crate::probe_helpers::{is_atmos_audio, subtitle_infos};
use crate::track_policy::{describe_role, enabled_track_ids, roles_from_probe};

/// Why a rule could not reach a verdict. [`std::fmt::Display`] is the text after `rule crashed: `.
#[derive(Debug)]
pub enum RuleError {
    Tool(ToolError),
    Atom(AtomError),
    Parse(ParseError),
    Mp4(Mp4Error),
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tool(e) => write!(f, "{e}"),
            Self::Atom(e) => write!(f, "{e}"),
            Self::Parse(e) => write!(f, "{e}"),
            Self::Mp4(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RuleError {}

macro_rules! from_error {
    ($($source:ty => $variant:ident),*) => {$(
        impl From<$source> for RuleError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    )*};
}
from_error!(ToolError => Tool, AtomError => Atom, ParseError => Parse, Mp4Error => Mp4);

pub type RuleResult = Result<Vec<Violation>, RuleError>;

/// One compliance check.
pub trait Rule {
    fn name(&self) -> &'static str;
    fn severity(&self) -> Severity;

    /// The rule's findings for `path`.
    ///
    /// # Errors
    ///
    /// [`RuleError`] when the rule could not read what it needs.
    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult;
}

fn violation(rule: &dyn Rule, message: impl Into<String>, track_id: Option<u32>) -> Violation {
    Violation {
        rule: rule.name().to_owned(),
        severity: rule.severity(),
        message: message.into(),
        track_id,
    }
}

fn field<'a>(track: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    track.get(key).map_or("", String::as_str)
}

/// `dby1` only with real Atmos, and real Atmos only with `dby1`.
pub struct FtypBrandsRule;

impl FtypBrandsRule {
    /// Whether ffprobe sees Atmos (JOC) in any audio stream; `None` when ffprobe fails.
    fn stream_atmos(audit: &Audit<'_>, path: &Path) -> Option<bool> {
        let probe = audit.probe(path).ok()?;
        let streams = probe.get("streams").and_then(serde_json::Value::as_array)?;
        Some(
            streams
                .iter()
                .filter(|s| {
                    s.get("codec_type").and_then(serde_json::Value::as_str) == Some("audio")
                })
                .any(is_atmos_audio),
        )
    }
}

impl Rule for FtypBrandsRule {
    fn name(&self) -> &'static str {
        "ftyp-dby1-requires-atmos"
    }

    fn severity(&self) -> Severity {
        Severity::Blocker
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let info_text = audit.tool_output("mp4info", path)?;
        let dump_text = audit.tool_output("mp4dump", path)?;
        let brands = parse_ftyp_brands(&dump_text);
        let tracks = parse_mp4info_tracks(&info_text)?;
        let has_dby1 = brands.iter().any(|b| b == "dby1");
        let dec3_atmos = tracks.iter().any(|t| {
            field(t, "type").to_lowercase() == "audio" && t.get("atmos").is_some_and(|v| v == "yes")
        });
        // One signal with prepare and detect_issues: the bitstream as ffprobe reads it. The dec3
        // flag is only the fallback when ffprobe fails.
        let has_real_atmos = Self::stream_atmos(audit, path).unwrap_or(dec3_atmos);

        // dby1 without real Atmos: the receiver locks to Atmos mode, no JOC arrives, silence.
        if has_dby1 && !has_real_atmos {
            return Ok(vec![violation(
                self,
                "ftyp compatible_brands contains 'dby1' but no audio track reports real Dolby Atmos \
                 (JOC); AVPlayer will route via Dolby MAT and deliver silence on Atmos-capable AVRs.",
                None,
            )]);
        }
        // Real Atmos without dby1: AVPlayer may not enable Atmos passthrough.
        if has_real_atmos && !has_dby1 {
            return Ok(vec![violation(
                self,
                "an audio track reports real Dolby Atmos (JOC) but ftyp compatible_brands is missing \
                 'dby1'; AVPlayer may not enable Atmos passthrough to the receiver.",
                None,
            )]);
        }
        // The dec3 box claims Atmos the stream does not carry; prepare clears the flag in place.
        if dec3_atmos && !has_real_atmos {
            return Ok(vec![violation(
                self,
                "an audio track's dec3 box claims Dolby Atmos (JOC) but the stream carries none; \
                 AVPlayer would announce Atmos the receiver never gets.",
                None,
            )]);
        }
        Ok(Vec::new())
    }
}

/// At most one ENABLED track per media type.
pub struct SingleEnabledPerTypeRule;

impl Rule for SingleEnabledPerTypeRule {
    fn name(&self) -> &'static str {
        "single-enabled-per-type"
    }

    fn severity(&self) -> Severity {
        Severity::Blocker
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let tracks = parse_mp4info_tracks(&audit.tool_output("mp4info", path)?)?;
        let mut buckets: [(&str, Vec<&BTreeMap<String, String>>); 3] = [
            ("video", Vec::new()),
            ("audio", Vec::new()),
            ("subtitle", Vec::new()),
        ];
        for track in &tracks {
            if !flags_enabled(field(track, "flags")) {
                continue;
            }
            match field(track, "type").to_lowercase().as_str() {
                "video" => buckets[0].1.push(track),
                "audio" => buckets[1].1.push(track),
                "subtitles" | "text" => buckets[2].1.push(track),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for (media, enabled) in &buckets {
            if enabled.len() > 1 {
                let ids: Vec<&str> = enabled
                    .iter()
                    .map(|t| {
                        t.get("id")
                            .or_else(|| t.get("track_no"))
                            .map_or("?", String::as_str)
                    })
                    .collect();
                found.push(violation(
                    self,
                    format!(
                        "{} {media} tracks carry the ENABLED flag (track ids: {}); AVPlayer selection becomes unpredictable.",
                        enabled.len(),
                        ids.join(", ")
                    ),
                    None,
                ));
            }
        }
        Ok(found)
    }
}

/// Subtitle `hdlr.handler_type` must be `sbtl`, not `text`.
pub struct SubtitleHandlerTypeRule;

impl Rule for SubtitleHandlerTypeRule {
    fn name(&self) -> &'static str {
        "subtitle-handler-type"
    }

    fn severity(&self) -> Severity {
        Severity::Blocker
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let handlers = parse_track_handlers(&audit.tool_output("mp4dump", path)?);
        let chapters = chapter_track_ids(path)?;
        let mut found = Vec::new();
        for handler in handlers {
            // vide / soun are fine; only 'text' is flagged, and a chapter track keeps 'text'.
            let is_chapter = handler
                .track_id
                .and_then(|id| u32::try_from(id).ok())
                .is_some_and(|id| chapters.contains(&id));
            if handler.handler_type == "text" && !is_chapter {
                found.push(violation(
                    self,
                    format!(
                        "track #{} uses hdlr.handler_type='text'; AVPlayer silently ignores this track. \
                         Remux with handler_type='sbtl'.",
                        handler.track_index
                    ),
                    Some(handler.track_index),
                ));
            }
        }
        Ok(found)
    }
}

/// Fragmented MP4 (`mvex` inside `moov`) breaks `AVPlayer` over progressive HTTP.
pub struct NoFragmentsRule;

impl Rule for NoFragmentsRule {
    fn name(&self) -> &'static str {
        "no-fragments"
    }

    fn severity(&self) -> Severity {
        Severity::Blocker
    }

    fn check(&self, _audit: &Audit<'_>, path: &Path) -> RuleResult {
        if !is_fragmented(path)? {
            return Ok(Vec::new());
        }
        Ok(vec![violation(
            self,
            "file is fragmented (mvex atom found in moov) -- AVPlayer on tvOS mishandles fragmented MP4 \
             over progressive HTTP",
            None,
        )])
    }
}

/// `moov` must precede `mdat` for smooth progressive HTTP playback.
pub struct FastStartRule;

impl Rule for FastStartRule {
    fn name(&self) -> &'static str {
        "fast-start"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, _audit: &Audit<'_>, path: &Path) -> RuleResult {
        // 'moov' first is faststart; neither present is not this rule's problem.
        if matches!(first_moov_vs_mdat(path)?, Some(Top::Mdat)) {
            return Ok(vec![violation(
                self,
                "moov atom appears after mdat -- progressive HTTP streaming will buffer slowly",
                None,
            )]);
        }
        Ok(Vec::new())
    }
}

fn track_id_number(track_id: &str) -> Option<u32> {
    track_id.parse().ok()
}

/// H.264 tracks must stay at Level 5.2 (52) or below.
pub struct AvcLevelRule;

impl Rule for AvcLevelRule {
    fn name(&self) -> &'static str {
        "avc-level-atv-limit"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let infos = parse_video_codec_infos(&audit.tool_output("mp4info", path)?);
        let mut found = Vec::new();
        for info in infos
            .iter()
            .filter(|i| i.coding == "avc1" || i.coding == "avc3")
        {
            let Some(level) = &info.level else {
                continue;
            };
            // The level is a run of digits; compare by length first so no number size matters.
            let digits = level.trim_start_matches('0');
            let shown = if digits.is_empty() { "0" } else { digits };
            let exceeds = shown.len() > 2 || shown.parse::<u32>().is_ok_and(|n| n > 52);
            if exceeds {
                found.push(violation(
                    self,
                    format!(
                        "H.264 track id={} is Level {shown}, Apple TV max is Level 5.2 (52)",
                        info.track_id
                    ),
                    track_id_number(&info.track_id),
                ));
            }
        }
        Ok(found)
    }
}

/// HEVC tracks must stay at Level 5.2 or below.
pub struct HevcLevelRule;

impl Rule for HevcLevelRule {
    fn name(&self) -> &'static str {
        "hevc-level-atv-limit"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let infos = parse_video_codec_infos(&audit.tool_output("mp4info", path)?);
        let mut found = Vec::new();
        for info in infos
            .iter()
            .filter(|i| ["hvc1", "hev1", "dvh1", "dvhe"].contains(&i.coding.as_str()))
        {
            if info.level.as_deref().is_some_and(hevc_level_exceeds_52) {
                found.push(violation(
                    self,
                    format!(
                        "HEVC track id={} level exceeds Apple TV 4K limit (5.2)",
                        info.track_id
                    ),
                    track_id_number(&info.track_id),
                ));
            }
        }
        Ok(found)
    }
}

/// AC-3 / E-AC-3 at 48 kHz; AAC at 44.1 or 48 kHz.
pub struct AudioSampleRateRule;

impl Rule for AudioSampleRateRule {
    fn name(&self) -> &'static str {
        "audio-sample-rate"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let infos = parse_audio_codec_infos(&audit.tool_output("mp4info", path)?);
        let mut found = Vec::new();
        for info in &infos {
            let Some(rate) = info.sample_rate else {
                continue;
            };
            if info.coding.is_empty() {
                continue;
            }
            let id = track_id_number(&info.track_id);
            if ["ac-3", "ac3", "ec-3", "ec3"].contains(&info.coding.as_str()) {
                if rate != 48000 {
                    found.push(violation(
                        self,
                        format!(
                            "{} track id={} sample rate {rate}Hz (expected 48000)",
                            info.coding, info.track_id
                        ),
                        id,
                    ));
                }
            } else if info.coding == "mp4a" && rate != 44100 && rate != 48000 {
                found.push(violation(
                    self,
                    format!(
                        "{} track id={} sample rate {rate}Hz (expected 44100 or 48000)",
                        info.coding, info.track_id
                    ),
                    id,
                ));
            }
        }
        Ok(found)
    }
}

fn count(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Plain-language findings for what a file still lacks; empty when nothing needs patching.
#[must_use]
pub fn tag_findings(check: &TagCheck) -> Vec<String> {
    if !check.needs_patch {
        return Vec::new();
    }
    let (have, want) = (check.current, check.wanted);
    let mut findings = Vec::new();
    let mut push = |n: u32, one: &str, many: &str| {
        if n > 0 {
            findings.push(count(n, one, many));
        }
    };
    push(
        want.elng.saturating_sub(have.elng),
        "track lacks an elng tag",
        "tracks lack an elng tag",
    );
    push(
        want.fall.saturating_sub(have.fall),
        "surround track has no stereo fallback",
        "surround tracks have no stereo fallback",
    );
    push(
        want.folw.saturating_sub(have.folw),
        "audio track does not follow its forced subtitle track",
        "audio tracks do not follow their forced subtitle track",
    );
    push(
        want.forc.saturating_sub(have.forc),
        "subtitle track is not linked to its forced variant",
        "subtitle tracks are not linked to their forced variant",
    );
    push(
        want.forced_flags.saturating_sub(have.forced_flags),
        "forced subtitle track lacks the forced display flags",
        "forced subtitle tracks lack the forced display flags",
    );
    if findings.is_empty() {
        findings.push("track references differ from the expected layout".to_owned());
    }
    findings
}

/// The elng language tags, track links and forced flags Apple's own files carry.
pub struct AppleTagsRule;

impl Rule for AppleTagsRule {
    fn name(&self) -> &'static str {
        "apple-tags"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        // Without ffprobe only elng and fall can be judged.
        let infos = audit
            .probe(path)
            .map(|probe| subtitle_infos(&probe))
            .unwrap_or_default();
        let check = check_accessibility(path, &infos)?;
        Ok(tag_findings(&check)
            .into_iter()
            .map(|message| violation(self, message, None))
            .collect())
    }
}

/// The ENABLED tracks must match the selection Apple's own iTunes files use.
pub struct DefaultSelectionRule;

impl Rule for DefaultSelectionRule {
    fn name(&self) -> &'static str {
        "default-track-selection"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        // A warning rule must not turn a probe failure into a blocker.
        let Ok(probe) = audit.probe(path) else {
            return Ok(Vec::new());
        };
        let roles = roles_from_probe(&probe);
        let known: BTreeSet<u32> = roles.iter().map(|r| r.track_id).collect();
        let tracks = parse_mp4info_tracks(&audit.tool_output("mp4info", path)?)?;
        let actual: BTreeSet<u32> = tracks
            .iter()
            .filter(|t| flags_enabled(field(t, "flags")))
            .filter_map(|t| field(t, "id").parse::<u32>().ok())
            .filter(|id| known.contains(id))
            .collect();
        let expected = enabled_track_ids(&roles);
        if actual == expected {
            return Ok(Vec::new());
        }
        // Name the tracks by kind (language, layout, forced), not by id.
        let by_id: BTreeMap<u32, _> = roles.iter().map(|r| (r.track_id, r)).collect();
        let kinds = |ids: Vec<&u32>| {
            ids.iter()
                .map(|id| describe_role(by_id[*id]))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut parts = Vec::new();
        let enable: Vec<&u32> = expected.difference(&actual).collect();
        let disable: Vec<&u32> = actual.difference(&expected).collect();
        if !enable.is_empty() {
            parts.push(format!("enable {}", kinds(enable)));
        }
        if !disable.is_empty() {
            parts.push(format!("disable {}", kinds(disable)));
        }
        Ok(vec![violation(
            self,
            format!("track selection differs from Apple's: {}", parts.join("; ")),
            None,
        )])
    }
}

/// Every language with a surround track needs a stereo twin.
pub struct StereoTwinRule;

impl Rule for StereoTwinRule {
    fn name(&self) -> &'static str {
        "stereo-twin"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let Ok(probe) = audit.probe(path) else {
            return Ok(Vec::new());
        };
        Ok(stereo_twin_gaps(&probe, &BTreeSet::new())
            .into_iter()
            .map(|lang| violation(self, format!("no stereo track for {lang}"), None))
            .collect())
    }
}

/// An audio stream whose packets duplicate an earlier stream.
pub struct DuplicateAudioRule;

impl Rule for DuplicateAudioRule {
    fn name(&self) -> &'static str {
        "duplicate-audio"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let Ok(probe) = audit.probe(path) else {
            return Ok(Vec::new());
        };
        Ok(audit
            .duplicate_audio(path, &probe)
            .into_iter()
            .map(|index| {
                violation(
                    self,
                    format!("audio stream #{index} duplicates an earlier stream"),
                    None,
                )
            })
            .collect())
    }
}

/// A video stream the `AVPlayer` cannot play and that therefore needs a re-encode.
pub struct VideoReencodeRule;

impl Rule for VideoReencodeRule {
    fn name(&self) -> &'static str {
        "video-reencode"
    }

    fn severity(&self) -> Severity {
        Severity::Warning
    }

    fn check(&self, audit: &Audit<'_>, path: &Path) -> RuleResult {
        let Ok(probe) = audit.probe(path) else {
            return Ok(Vec::new());
        };
        Ok(video_reencode_reason(&probe, false, false)
            .into_iter()
            .map(|reason| violation(self, reason, None))
            .collect())
    }
}

/// The built-in rule set: blockers first, then the warning rules.
#[must_use]
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(FtypBrandsRule),
        Box::new(SingleEnabledPerTypeRule),
        Box::new(SubtitleHandlerTypeRule),
        Box::new(NoFragmentsRule),
        Box::new(FastStartRule),
        Box::new(AvcLevelRule),
        Box::new(HevcLevelRule),
        Box::new(AudioSampleRateRule),
        Box::new(AppleTagsRule),
        Box::new(DefaultSelectionRule),
        Box::new(StereoTwinRule),
        Box::new(DuplicateAudioRule),
        Box::new(VideoReencodeRule),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use atvprep_testkit::{fixture_path, load_golden};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;
    use crate::compliance::tools::{Audit, ToolError, Tools};
    use crate::mp4::tags::{PatchSummary, TagCheck};

    #[derive(Deserialize)]
    struct Ignored {}

    fn texts() -> std::collections::BTreeMap<String, String> {
        load_golden::<Ignored, String>("compliance_texts")
            .into_iter()
            .map(|c| (c.name, c.expected))
            .collect()
    }

    /// Answers from recorded text, like the oracle's patched Python functions.
    struct Recorded {
        info: Option<String>,
        dump: Option<String>,
        probe: Option<Value>,
        duplicates: BTreeSet<i64>,
    }

    impl Tools for Recorded {
        fn run_tool(&self, name: &str, _path: &Path) -> std::result::Result<String, ToolError> {
            let text = if name == "mp4info" {
                &self.info
            } else {
                &self.dump
            };
            match text.as_deref() {
                Some(text) if !text.is_empty() => Ok(text.to_owned()),
                _ => Err(ToolError(format!(
                    "{name} produced no output for <path> (exit=0): "
                ))),
            }
        }

        fn ffprobe(&self, _path: &Path) -> std::result::Result<Value, ToolError> {
            self.probe
                .clone()
                .ok_or_else(|| ToolError("ffprobe failed".to_owned()))
        }

        fn duplicate_audio(&self, _path: &Path, _probe: &Value) -> BTreeSet<i64> {
            self.duplicates.clone()
        }
    }

    #[derive(Deserialize)]
    struct Input {
        file: String,
        info: Option<String>,
        dump: Option<String>,
        probe: Option<Value>,
        duplicates: BTreeSet<i64>,
    }

    #[test]
    fn rules_match_the_python_oracle() {
        let texts = texts();
        let cases = load_golden::<Input, Vec<Value>>("compliance_rules");
        assert!(cases.len() > 250);
        let rules = all_rules();
        for case in cases {
            let input = &case.input;
            let tools = Recorded {
                info: input.info.as_ref().map(|k| texts[k].clone()),
                dump: input.dump.as_ref().map(|k| texts[k].clone()),
                probe: input.probe.clone(),
                duplicates: input.duplicates.clone(),
            };
            let audit = Audit::new(&tools);
            let path = fixture_path(&input.file);
            for want in &case.expected {
                let name = want["rule"].as_str().unwrap();
                let rule = rules
                    .iter()
                    .find(|r| r.name() == name)
                    .unwrap_or_else(|| panic!("no rule {name}"));
                let label = format!("{} / {name}", case.name);
                match (rule.check(&audit, &path), want.get("raises")) {
                    (Ok(found), None) => {
                        let actual: Vec<Value> = found
                            .iter()
                            .map(|v| json!([v.rule, v.severity.as_str(), v.message, v.track_id]))
                            .collect();
                        assert_eq!(Value::Array(actual), want["violations"], "{label}");
                    }
                    (Err(error), Some(kind)) => {
                        assert_eq!(
                            json!(error.to_string()),
                            want["message"],
                            "{label} ({kind})"
                        );
                    }
                    (other, expected) => {
                        panic!("{label}: got {other:?}, Python raised {expected:?}")
                    }
                }
            }
        }
    }

    #[test]
    fn the_rule_order_is_blockers_then_warnings() {
        let rules = all_rules();
        let names: Vec<&str> = rules.iter().map(|r| r.name()).collect();
        assert_eq!(
            names,
            [
                "ftyp-dby1-requires-atmos",
                "single-enabled-per-type",
                "subtitle-handler-type",
                "no-fragments",
                "fast-start",
                "avc-level-atv-limit",
                "hevc-level-atv-limit",
                "audio-sample-rate",
                "apple-tags",
                "default-track-selection",
                "stereo-twin",
                "duplicate-audio",
                "video-reencode",
            ]
        );
        assert!(rules[..4].iter().all(|r| r.severity() == Severity::Blocker));
        assert!(rules[4..].iter().all(|r| r.severity() == Severity::Warning));
    }

    fn check(have: PatchSummary, want: PatchSummary, needs_patch: bool) -> TagCheck {
        TagCheck {
            current: have,
            wanted: want,
            needs_patch,
        }
    }

    #[test]
    fn tag_findings_pluralize_like_python() {
        let none = PatchSummary::default();
        assert!(tag_findings(&check(none, PatchSummary { elng: 2, ..none }, false)).is_empty());
        let want = PatchSummary {
            elng: 2,
            fall: 1,
            folw: 2,
            forc: 1,
            forced_flags: 1,
        };
        assert_eq!(
            tag_findings(&check(none, want, true)),
            [
                "2 tracks lack an elng tag",
                "1 surround track has no stereo fallback",
                "2 audio tracks do not follow their forced subtitle track",
                "1 subtitle track is not linked to its forced variant",
                "1 forced subtitle track lacks the forced display flags",
            ]
        );
        assert_eq!(
            tag_findings(&check(want, want, true)),
            ["track references differ from the expected layout"]
        );
        let one = PatchSummary { elng: 1, ..none };
        assert_eq!(
            tag_findings(&check(none, one, true)),
            ["1 track lacks an elng tag"]
        );
    }

    #[test]
    fn a_warning_rule_swallows_probe_failures() {
        let tools = Recorded {
            info: None,
            dump: None,
            probe: None,
            duplicates: BTreeSet::new(),
        };
        let audit = Audit::new(&tools);
        let path = fixture_path("plain_faststart.mp4");
        for rule in [
            &StereoTwinRule as &dyn Rule,
            &DuplicateAudioRule,
            &VideoReencodeRule,
            &DefaultSelectionRule,
        ] {
            assert_eq!(
                rule.check(&audit, &path).unwrap(),
                Vec::new(),
                "{}",
                rule.name()
            );
        }
    }

    #[test]
    fn a_blocker_rule_reports_a_missing_tool_instead_of_passing() {
        let tools = Recorded {
            info: None,
            dump: None,
            probe: None,
            duplicates: BTreeSet::new(),
        };
        let audit = Audit::new(&tools);
        let error = SingleEnabledPerTypeRule
            .check(&audit, &fixture_path("plain_faststart.mp4"))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "mp4info produced no output for <path> (exit=0): "
        );
    }
}

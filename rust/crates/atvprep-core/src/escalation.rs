//! Decide per file how far `prepare` has to go (port of `atvprep/escalation.py`); this part holds
//! the helpers that are pure over an ffprobe result.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::analyzers::{analyze_audio, analyze_video};
use crate::compliance::Audit;
use crate::ffprobe::Ffprobe;
use crate::models::VideoAction;
use crate::mp4::patch::check_accessibility;
use crate::output::describe_patch;
use crate::planner::{DetectOptions, detect_issues, stereo_gaps};
use crate::probe_helpers::{ProbeContext, subtitle_infos};
use crate::verify::verify_file_in;

/// The CRF the planner uses when it has to re-encode video (the analysis does not use it).
const REENCODE_CRF: u32 = 18;

fn streams_of<'a>(probe: &'a Value, kind: &'a str) -> impl Iterator<Item = &'a Value> {
    probe
        .get("streams")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(move |s| s.get("codec_type").and_then(Value::as_str) == Some(kind))
}

/// Why the first video stream needs a re-encode to HEVC, or `None` when it can be copied.
#[must_use]
pub fn video_reencode_reason(probe: &Value, prefer_hevc10: bool, keep_av1: bool) -> Option<String> {
    let video = streams_of(probe, "video").next()?;
    let info = analyze_video(video, prefer_hevc10, REENCODE_CRF, keep_av1);
    if info.action != VideoAction::Transcode {
        return None;
    }
    let shown = if info.profile.is_empty() {
        &info.pix_fmt
    } else {
        &info.profile
    };
    Some(format!(
        "video {} ({shown}) needs a re-encode to HEVC",
        info.codec
    ))
}

/// Languages (sorted) whose surround track has no stereo twin, so the planner adds a downmix.
#[must_use]
pub fn stereo_twin_gaps(probe: &Value, skip: &BTreeSet<i64>) -> Vec<String> {
    let audio: Vec<Value> = streams_of(probe, "audio").cloned().collect();
    stereo_gaps(&analyze_audio(&audio, skip))
}

/// How far `prepare` has to go for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The file already matches the target.
    None,
    /// Only flags, links or tags differ: a size-neutral postfix plus the tag patch.
    InPlace,
    /// Container, track layout or faststart are wrong: rebuild without a video encode.
    Rebuild,
    /// The video cannot be played as it is: re-encode it.
    Reencode,
}

impl Level {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::InPlace => "in_place",
            Self::Rebuild => "rebuild",
            Self::Reencode => "reencode",
        }
    }
}

/// The verdict of [`assess`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub level: Level,
    pub reasons: Vec<String>,
    pub duplicates: BTreeSet<i64>,
    /// `InPlace` and the only finding is the tag patch.
    pub tags_only: bool,
}

/// The switches that move the verdict.
#[derive(Debug, Clone, Copy, Default)]
pub struct AssessOptions {
    pub fix_dts: bool,
    pub prefer_hevc10: bool,
    pub keep_av1: bool,
    pub force: bool,
}

/// Classify `path`: re-encode, rebuild, patch in place, or leave alone.
///
/// One `audit` serves the whole assessment, so the duplicate-audio read and the tool outputs of the
/// verify inside it are shared.
#[must_use]
pub fn assess(
    audit: &Audit<'_>,
    path: &Path,
    probe: &Value,
    sidecars: &[PathBuf],
    ctx: &ProbeContext,
    options: &AssessOptions,
) -> Assessment {
    let duplicates = audit.duplicate_audio(path, probe);

    if let Some(reason) = video_reencode_reason(probe, options.prefer_hevc10, options.keep_av1) {
        return Assessment {
            level: Level::Reencode,
            reasons: vec![reason],
            duplicates,
            tags_only: false,
        };
    }

    // A missing audio language is no reason to rebuild: the remux writes `und` again, so the file
    // would be rebuilt on every run.
    let ffprobe = Ffprobe::new();
    let issues = detect_issues(
        path,
        probe,
        sidecars,
        Some(ctx),
        &ffprobe,
        &duplicates,
        DetectOptions {
            fix_dts: options.fix_dts,
            untagged_audio: false,
        },
    );
    if !issues.is_empty() || options.force {
        return Assessment {
            level: Level::Rebuild,
            reasons: issues,
            duplicates,
            tags_only: false,
        };
    }

    let report = verify_file_in(audit, path, None, None);
    let mut findings: Vec<String> = report
        .blockers()
        .iter()
        .map(|v| format!("{}: {}", v.rule, v.message))
        .collect();
    findings.extend(
        report
            .warnings()
            .iter()
            .filter(|v| v.rule == "default-track-selection")
            .map(|v| format!("{}: {}", v.rule, v.message)),
    );
    let from_verify = findings.len();

    if let Ok(tags) = check_accessibility(path, &subtitle_infos(probe))
        && tags.needs_patch
    {
        findings.push(format!(
            "tags: {} → {}",
            describe_patch(&tags.current),
            describe_patch(&tags.wanted)
        ));
    }
    if findings.is_empty() {
        return Assessment {
            level: Level::None,
            reasons: Vec::new(),
            duplicates,
            tags_only: false,
        };
    }
    Assessment {
        level: Level::InPlace,
        reasons: findings,
        duplicates,
        tags_only: from_verify == 0,
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
    struct Input {
        #[serde(rename = "fn")]
        func: String,
        probe: Value,
        #[serde(default)]
        prefer_hevc10: bool,
        #[serde(default)]
        keep_av1: bool,
        #[serde(default)]
        skip: BTreeSet<i64>,
    }

    #[test]
    fn helpers_match_the_python_oracle() {
        let cases = load_golden::<Input, Value>("escalation_helpers");
        assert!(cases.len() > 80);
        for case in cases {
            let input = &case.input;
            let actual = if input.func == "reencode" {
                json!(video_reencode_reason(
                    &input.probe,
                    input.prefer_hevc10,
                    input.keep_av1
                ))
            } else {
                json!(stereo_twin_gaps(&input.probe, &input.skip))
            };
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn a_probe_without_video_needs_no_reencode() {
        assert_eq!(video_reencode_reason(&json!({}), false, false), None);
    }

    mod assessment {
        use std::collections::BTreeSet;
        use std::path::{Path, PathBuf};

        use atvprep_testkit::{fixture_path, load_golden};
        use serde::Deserialize;
        use serde_json::{Value, json};

        use super::super::*;
        use crate::compliance::{Audit, ToolError, Tools};
        use crate::ffprobe::Ffprobe;
        use crate::planner::{DetectOptions, detect_issues};
        use crate::probe_helpers::ProbeContext;

        #[derive(Deserialize)]
        struct Ignored {}

        struct Recorded {
            info: String,
            dump: String,
            probe: Value,
            duplicates: BTreeSet<i64>,
        }

        impl Tools for Recorded {
            fn run_tool(&self, name: &str, _path: &Path) -> Result<String, ToolError> {
                Ok(if name == "mp4info" {
                    self.info.clone()
                } else {
                    self.dump.clone()
                })
            }

            fn ffprobe(&self, _path: &Path) -> Result<Value, ToolError> {
                Ok(self.probe.clone())
            }

            fn duplicate_audio(&self, _path: &Path, _probe: &Value) -> BTreeSet<i64> {
                self.duplicates.clone()
            }
        }

        #[derive(Deserialize)]
        struct Options {
            fix_dts: bool,
            prefer_hevc10: bool,
            keep_av1: bool,
            force: bool,
            dts: Option<i64>,
        }

        #[derive(Deserialize)]
        struct Input {
            file: String,
            ext: String,
            probe: Value,
            duplicates: BTreeSet<i64>,
            options: Options,
            sidecars: Vec<String>,
            info: String,
            dump: String,
        }

        #[test]
        fn detect_issues_and_assess_match_the_python_oracle() {
            let texts: std::collections::BTreeMap<String, String> =
                load_golden::<Ignored, String>("compliance_texts")
                    .into_iter()
                    .map(|c| (c.name, c.expected))
                    .collect();
            let cases = load_golden::<Input, Value>("assess");
            assert!(cases.len() > 400);
            for case in cases {
                let input = &case.input;
                let name = &case.name;
                let dir = tempfile::tempdir().unwrap();
                let stem = Path::new(&input.file)
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let path = dir.path().join(format!("{stem}{}", input.ext));
                std::fs::copy(fixture_path(&input.file), &path).unwrap();
                let sidecars: Vec<PathBuf> =
                    input.sidecars.iter().map(|n| dir.path().join(n)).collect();
                let faststart = [".mp4", ".m4v", ".mov"].contains(&input.ext.as_str())
                    && crate::mp4::patch::is_faststart(&path).unwrap();
                let ctx = ProbeContext {
                    infile: path.clone(),
                    ffprobe: input.probe.clone(),
                    first_video_dts: input.options.dts,
                    is_faststart: faststart,
                };
                let ffprobe = Ffprobe::new();
                let detect = |untagged_audio| {
                    detect_issues(
                        &path,
                        &input.probe,
                        &sidecars,
                        Some(&ctx),
                        &ffprobe,
                        &input.duplicates,
                        DetectOptions {
                            fix_dts: input.options.fix_dts,
                            untagged_audio,
                        },
                    )
                };
                assert_eq!(
                    json!(detect(true)),
                    case.expected["detect"],
                    "{name} detect"
                );
                assert_eq!(
                    json!(detect(false)),
                    case.expected["detect_quiet"],
                    "{name} detect_quiet"
                );
                assert_eq!(
                    json!(faststart),
                    case.expected["faststart"],
                    "{name} faststart"
                );

                let tools = Recorded {
                    info: texts[&input.info].clone(),
                    dump: texts[&input.dump].clone(),
                    probe: input.probe.clone(),
                    duplicates: input.duplicates.clone(),
                };
                let audit = Audit::new(&tools);
                let options = AssessOptions {
                    fix_dts: input.options.fix_dts,
                    prefer_hevc10: input.options.prefer_hevc10,
                    keep_av1: input.options.keep_av1,
                    force: input.options.force,
                };
                let found = assess(&audit, &path, &input.probe, &sidecars, &ctx, &options);
                let want = &case.expected["assess"];
                assert_eq!(json!(found.level.as_str()), want["level"], "{name} level");
                assert_eq!(json!(found.reasons), want["reasons"], "{name} reasons");
                assert_eq!(
                    json!(found.duplicates),
                    want["duplicates"],
                    "{name} duplicates"
                );
                assert_eq!(
                    json!(found.tags_only),
                    want["tags_only"],
                    "{name} tags_only"
                );
            }
        }

        #[test]
        fn a_non_mp4_extension_stops_at_the_remux_reason() {
            let probe = json!({"streams": [], "format": {"format_name": "matroska,webm"}});
            let ffprobe = Ffprobe::new();
            let reasons = detect_issues(
                Path::new("/m/film.mkv"),
                &probe,
                &[],
                None,
                &ffprobe,
                &BTreeSet::new(),
                DetectOptions {
                    fix_dts: false,
                    untagged_audio: true,
                },
            );
            assert_eq!(reasons, ["source is .mkv (must remux to mp4)"]);
        }
    }
}

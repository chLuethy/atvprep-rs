//! Human-readable output of the plan, the patch summary and the post-run checks (port of
//! `atvprep/output.py`). Plain text: the facts are the same, the box drawing and colours are not.

use std::path::Path;

use serde_json::Value;

use crate::ffprobe::Ffprobe;
use crate::models::{AudioAction, Plan, SubAction, SubSource, VideoAction};
use crate::mp4::tags::{PatchSummary, elng_tag};
use crate::probe_helpers::is_atmos_audio;
use crate::tracklinks::{AudioRef, SubRef, TrackLinks, plan_links as link_policy};

const LINKS_LEGEND: &str = "Links: A# = audio, S# = subtitle track · fall = falls back to the stereo track · \
folw = follows the forced subtitle · forc = forced variant of a full subtitle";

/// The track links the patch step will write for this plan (the same policy as `mp4patch`).
#[must_use]
pub fn plan_links(plan: &Plan) -> TrackLinks {
    let language = |lang: &str| (lang != "und").then(|| lang.to_owned());
    let audio: Vec<AudioRef> = plan
        .audio_tracks
        .iter()
        .map(|t| AudioRef {
            lang: language(&t.lang),
            channels: t.channels,
        })
        .collect();
    let subs: Vec<SubRef> = plan
        .sub_tracks
        .iter()
        .filter(|t| t.action != SubAction::Skip)
        .map(|t| SubRef {
            lang: language(&t.lang),
            forced: t.forced,
        })
        .collect();
    link_policy(&audio, &subs)
}

/// `elng 5 · fall 1 · …`: only the non-zero parts, or `no tags`.
#[must_use]
pub fn describe_patch(summary: &PatchSummary) -> String {
    let parts: Vec<String> = [
        ("elng", summary.elng),
        ("fall", summary.fall),
        ("folw", summary.folw),
        ("forc", summary.forc),
        ("forced", summary.forced_flags),
    ]
    .iter()
    .filter(|(_, value)| *value > 0)
    .map(|(name, value)| format!("{name} {value}"))
    .collect();
    if parts.is_empty() {
        "no tags".to_owned()
    } else {
        parts.join(" · ")
    }
}

/// One line on what the patched file carries; with a plan, flag links it failed to get.
#[must_use]
pub fn patch_summary_lines(summary: &PatchSummary, plan: Option<&Plan>) -> Vec<String> {
    let mut out = vec![format!("  tags: {}", describe_patch(summary))];
    let Some(plan) = plan else {
        return out;
    };
    let track_links = plan_links(plan);
    let counted = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let planned = PatchSummary {
        elng: 0,
        fall: counted(track_links.fall.len()),
        folw: counted(track_links.folw.len()),
        forc: counted(track_links.forc.len()),
        forced_flags: counted(
            plan.sub_tracks
                .iter()
                .filter(|t| t.action != SubAction::Skip && t.forced)
                .count(),
        ),
    };
    let written = PatchSummary {
        elng: 0,
        ..*summary
    };
    if written != planned {
        out.push(format!(
            "  ! file differs from plan: planned {}",
            describe_patch(&planned)
        ));
    }
    out
}

fn lang_cell(lang: &str) -> String {
    match elng_tag(lang) {
        Some(tag) if tag != lang => format!("{lang} {tag}"),
        _ => lang.to_owned(),
    }
}

fn links_cell(labels: &[String]) -> String {
    if labels.is_empty() {
        "–".to_owned()
    } else {
        labels.join(" · ")
    }
}

/// Left-aligned columns separated by two spaces, indented by two.
fn table(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    format!(
                        "{cell}{}",
                        " ".repeat(widths[c].saturating_sub(cell.chars().count()))
                    )
                })
                .collect();
            format!("  {}", cells.join("  ").trim_end())
        })
        .collect()
}

fn s(text: &str) -> String {
    text.to_owned()
}

fn video_section(plan: &Plan) -> Vec<String> {
    let Some(vi) = &plan.video_info else {
        return Vec::new();
    };
    let tag = vi
        .tag
        .as_ref()
        .map_or_else(String::new, |t| format!(" ({t})"));
    let profile = if vi.profile.is_empty() {
        String::new()
    } else {
        format!(" [{}]", vi.profile)
    };
    let dv = if vi.dolby_vision { " [DV]" } else { "" };
    let action = if vi.action == VideoAction::Transcode {
        "transcode → HEVC 10-bit"
    } else {
        "copy"
    };
    let mut section = vec![s(" VIDEO")];
    section.extend(table(&[
        vec![
            s("Codec"),
            format!("{}{tag}{profile}{dv}", vi.codec),
            s("Action"),
            s(action),
        ],
        vec![
            s("Resolution"),
            format!("{}×{}", vi.width, vi.height),
            s("HDR"),
            s(if vi.hdr { "HDR" } else { "SDR" }),
        ],
    ]));
    section
}

fn audio_section(plan: &Plan, track_links: &TrackLinks) -> Vec<String> {
    if plan.audio_tracks.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![
        ["#", "Codec", "Ch", "Lang", "Atmos", "Action", "Links"]
            .iter()
            .map(|c| s(c))
            .collect::<Vec<_>>(),
    ];
    for (i, t) in plan.audio_tracks.iter().enumerate() {
        let mut action = match t.action {
            AudioAction::Copy => s("copy"),
            AudioAction::Downmix => s("downmix → AAC 2.0"),
            AudioAction::TranscodeEac3 => s("transcode → EAC3"),
        };
        if t.is_default {
            action.push_str(" (default)");
        }
        rows.push(vec![
            i.to_string(),
            t.codec.clone(),
            t.channels.to_string(),
            lang_cell(&t.lang),
            s(if t.is_atmos { "✓" } else { "–" }),
            action,
            links_cell(&track_links.audio_labels(i)),
        ]);
    }
    let mut section = vec![s(" AUDIO")];
    section.extend(table(&rows));
    section
}

fn subtitle_section(plan: &Plan, track_links: &TrackLinks) -> Vec<String> {
    let active: Vec<_> = plan
        .sub_tracks
        .iter()
        .filter(|t| t.action != SubAction::Skip)
        .collect();
    let skipped = plan.sub_tracks.len() - active.len();
    if active.is_empty() && skipped == 0 {
        return Vec::new();
    }
    let mut rows = vec![
        [
            "#", "Source", "Codec", "Lang", "Forced", "SDH", "Action", "Links",
        ]
        .iter()
        .map(|c| s(c))
        .collect::<Vec<_>>(),
    ];
    for (i, t) in active.iter().enumerate() {
        let mut action = s("mov_text");
        if t.is_default {
            action.push_str(" (default)");
        }
        let source = match t.source {
            SubSource::Internal => "internal",
            SubSource::External => "external",
        };
        rows.push(vec![
            i.to_string(),
            s(source),
            t.codec.clone(),
            lang_cell(&t.lang),
            s(if t.forced { "✓" } else { "–" }),
            s(if t.sdh { "✓" } else { "–" }),
            action,
            links_cell(&track_links.sub_labels(i)),
        ]);
    }
    if skipped > 0 {
        rows.push(vec![
            s(""),
            s(""),
            s(""),
            s(""),
            s(""),
            s(""),
            format!("… {skipped} image subtitle(s) skipped"),
            s(""),
        ]);
    }
    let mut section = vec![s(" SUBTITLES")];
    section.extend(table(&rows));
    section
}

/// The plan as plain text: title, VIDEO, AUDIO and SUBTITLES tables, the links legend and warnings.
#[must_use]
pub fn plan_table(plan: &Plan) -> Vec<String> {
    let track_links = plan_links(plan);
    let mut out = vec![
        String::new(),
        format!(
            "{}  →  {}",
            file_name(&plan.in_path),
            file_name(&plan.out_path)
        ),
    ];
    out.extend(video_section(plan));
    out.extend(audio_section(plan, &track_links));
    out.extend(subtitle_section(plan, &track_links));
    if !track_links.fall.is_empty() || !track_links.folw.is_empty() || !track_links.forc.is_empty()
    {
        out.push(format!(" {LINKS_LEGEND}"));
    }
    out.extend(plan.warnings.iter().map(|w| format!("  ! {w}")));
    if !plan.warnings.is_empty() {
        out.push(String::new());
    }
    out
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Check that the expected number of Atmos tracks made it into the output file.
#[must_use]
pub fn verify_atmos_in_output(
    out_path: &Path,
    expected_atmos_count: usize,
    ffprobe: &Ffprobe,
) -> Vec<String> {
    if expected_atmos_count == 0 {
        return Vec::new();
    }
    let Ok(probe) = ffprobe.probe_json(out_path) else {
        return vec![format!(
            "Post-verify: ffprobe failed for {}",
            file_name(out_path)
        )];
    };
    let found = probe
        .get("streams")
        .and_then(Value::as_array)
        .map_or(0, |streams| {
            streams
                .iter()
                .filter(|s| s.get("codec_type").and_then(Value::as_str) == Some("audio"))
                .filter(|s| is_atmos_audio(s))
                .count()
        });
    if found < expected_atmos_count {
        return vec![format!(
            "Post-verify: {expected_atmos_count} Atmos track(s) expected, only {found} found in output — JOC metadata may be lost."
        )];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use atvprep_testkit::load_golden;
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;
    use crate::models::Plan;

    #[derive(Deserialize)]
    struct Input {
        kind: String,
        summary: Option<Value>,
        plan: Option<Plan>,
    }

    fn summary_of(value: &Value) -> PatchSummary {
        let n = |key: &str| u32::try_from(value[key].as_u64().unwrap()).unwrap();
        PatchSummary {
            elng: n("elng"),
            fall: n("fall"),
            folw: n("folw"),
            forc: n("forc"),
            forced_flags: n("forced_flags"),
        }
    }

    #[test]
    fn output_matches_the_python_oracle() {
        let cases = load_golden::<Input, Value>("output");
        assert!(cases.len() > 100);
        for case in cases {
            let input = &case.input;
            let name = &case.name;
            match input.kind.as_str() {
                "describe" => {
                    let summary = summary_of(input.summary.as_ref().unwrap());
                    assert_eq!(json!(describe_patch(&summary)), case.expected, "{name}");
                }
                "summary" => {
                    let summary = summary_of(input.summary.as_ref().unwrap());
                    let lines = patch_summary_lines(&summary, input.plan.as_ref());
                    assert_eq!(json!(lines), case.expected, "{name}");
                }
                _ => {
                    let plan = input.plan.as_ref().unwrap();
                    let text = plan_table(plan).join("\n");
                    for token in case.expected.as_array().unwrap() {
                        let token = token.as_str().unwrap();
                        assert!(
                            text.contains(token),
                            "{name}: {token:?} missing from\n{text}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn plan_links_follow_the_patch_policy() {
        let case = load_golden::<Input, Value>("output")
            .into_iter()
            .find(|c| c.name.starts_with("table h264 copy with sidecar"))
            .unwrap();
        let plan = case.input.plan.unwrap();
        let links = plan_links(&plan);
        let audio = plan.audio_tracks.len();
        assert!(links.fall.keys().all(|&i| i < audio));
    }

    #[test]
    fn atmos_verification_messages_match_python() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            crate::ffprobe::Ffprobe::with_program(path)
        };
        let out = std::path::Path::new("/m/out.mp4");
        assert!(verify_atmos_in_output(out, 0, &script("none", "exit 1")).is_empty());
        assert_eq!(
            verify_atmos_in_output(out, 1, &script("broken", "exit 1")),
            ["Post-verify: ffprobe failed for out.mp4"]
        );
        let one = r#"printf '{"streams": [{"codec_type": "audio", "codec_name": "eac3", "profile": "Dolby Digital Plus + Dolby Atmos"}]}'"#;
        assert!(verify_atmos_in_output(out, 1, &script("one", one)).is_empty());
        assert_eq!(
            verify_atmos_in_output(out, 2, &script("two", one)),
            [
                "Post-verify: 2 Atmos track(s) expected, only 1 found in output — JOC metadata may be lost."
            ]
        );
    }
}

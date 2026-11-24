//! The target file after Prepare has chunks of about half a second (ADR 0010): every
//! ffmpeg run ends with `MP4Box`, then the moov patch runs. Real tools (ffmpeg, Bento4,
//! `MP4Box`); tests that need `MP4Box` are skipped when it is missing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use atvprep_core::compliance::{SystemTools, Tools};
use atvprep_core::console::{Console, SharedBuffer};
use atvprep_core::encode::EncodeAllowance;
use atvprep_core::ffprobe::Ffprobe;
use atvprep_core::hook::run_hook;
use atvprep_core::pipeline_postfix::RunContext;
use atvprep_core::prepare::chunks::Mp4Box;
use atvprep_core::prepare::pipeline::prepare_file;
use atvprep_core::prepare::{ErrorKind, Outcome, PrepareOptions};
use atvprep_core::stages::StageReporter;
use serde_json::{Value, json};

fn mp4box_installed() -> bool {
    Mp4Box::new().find().is_some()
}

/// Skips the test (with a note) when `MP4Box` is not installed.
macro_rules! needs_mp4box {
    () => {
        if !mp4box_installed() {
            eprintln!("skipped: MP4Box is not installed");
            return;
        }
    };
}

struct Run {
    _dir: tempfile::TempDir,
    base: PathBuf,
    ctx: RunContext,
    console: SharedBuffer,
    machine: SharedBuffer,
}

/// A run context; if `present` is false, it looks for `MP4Box` "nowhere".
fn run(present: bool) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let (console, console_log) = Console::recording(false);
    let machine = SharedBuffer::default();
    let mp4box = if present {
        Mp4Box::new()
    } else {
        Mp4Box::new().with_search_path(vec![base.join("no-tools-here")])
    };
    let ctx = RunContext {
        console,
        reporter: StageReporter::new(Box::new(machine.clone()), false),
        tools: Box::new(SystemTools::new()),
        ffprobe: Ffprobe::new(),
        ffmpeg: None,
        mp4box,
    };
    Run {
        _dir: dir,
        base,
        ctx,
        console: console_log,
        machine,
    }
}

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .stdin(Stdio::null())
        .args(["-v", "error", "-y"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg {args:?}");
}

/// Twelve seconds of H.264 and AC-3 5.1 at 44.1 kHz: ready for Apple TV except for the sample rate, so
/// Prepare rebuilds it with ffmpeg.
fn twelve_seconds(dir: &Path) -> PathBuf {
    let path = dir.join("Film.mp4");
    let out = path.to_str().unwrap();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=44100",
        "-t",
        "12",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-profile:v",
        "high",
        "-c:a",
        "ac3",
        "-ac",
        "6",
        "-ar",
        "44100",
        "-metadata:s:v:0",
        "language=deu",
        "-metadata:s:a:0",
        "language=deu",
        "-disposition:a:0",
        "default",
        "-movflags",
        "+faststart",
        out,
    ]);
    path
}

fn all_audio() -> PrepareOptions {
    PrepareOptions {
        encode: EncodeAllowance::Audio,
        ..PrepareOptions::default()
    }
}

/// Mean chunk duration in seconds per audio and video track, read from the moov alone.
fn mean_chunk_seconds(path: &Path) -> Vec<(String, f64)> {
    let dump = SystemTools::new().run_tool("mp4dump", path).unwrap();
    let mut tracks: Vec<(String, f64, f64, u64)> = Vec::new();
    let mut section = "";
    for line in dump.lines() {
        let text = line.trim();
        let value = |key: &str| {
            text.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(" = "))
                .map(str::to_owned)
        };
        if text.starts_with("[trak]") {
            tracks.push((String::new(), 0.0, 0.0, 0));
        } else if text.starts_with("[mdhd]") {
            section = "mdhd";
        } else if text.starts_with("[stco]") || text.starts_with("[co64]") {
            section = "chunks";
        } else if text.starts_with('[') {
            section = "";
        }
        let Some(track) = tracks.last_mut() else {
            continue;
        };
        if let Some(kind) = value("handler_type") {
            track.0 = kind;
        } else if section == "mdhd" {
            if let Some(v) = value("timescale") {
                track.1 = v.parse().unwrap();
            } else if let Some(v) = value("duration") {
                track.2 = v.parse::<f64>().unwrap() / track.1;
            }
        } else if section == "chunks"
            && let Some(v) = value("entry_count")
        {
            track.3 = v.parse().unwrap();
        }
    }
    tracks
        .into_iter()
        .filter(|(kind, ..)| kind == "vide" || kind == "soun")
        .map(|(kind, _, seconds, chunks)| {
            (
                kind,
                seconds / f64::from(u32::try_from(chunks.max(1)).unwrap()),
            )
        })
        .collect()
}

fn sha(path: &Path) -> String {
    atvprep_testkit::sha256_hex(&std::fs::read(path).unwrap())
}

#[test]
fn a_prepared_file_has_chunks_of_about_half_a_second() {
    needs_mp4box!();
    let mut r = run(true);
    let source = twelve_seconds(&r.base);
    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();
    assert_eq!(result.error, None);
    assert!(result.ok());

    let means = mean_chunk_seconds(&source);
    let kinds: Vec<&str> = means.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds.first(), Some(&"vide"), "{means:?}");
    assert!(
        kinds.len() >= 2 && kinds[1..].iter().all(|k| *k == "soun"),
        "{means:?}"
    );
    for (kind, mean) in &means {
        assert!(
            (0.4..=0.6).contains(mean),
            "{kind}: mean chunk duration {mean:.3} s"
        );
    }

    // The file is still the target of the moov patch: the moov is at the front.
    assert!(atvprep_core::mp4::patch::is_faststart(&source).unwrap());
}

#[test]
fn the_chunk_stage_reports_progress_between_encode_and_finalize() {
    needs_mp4box!();
    let mut r = run(true);
    let source = twelve_seconds(&r.base);
    prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();

    let lines: Vec<String> = r
        .machine
        .lines()
        .into_iter()
        .filter(|l| l.starts_with("ATVPREP-STAGE ") || l.starts_with("ATVPREP-PROGRESS "))
        .collect();
    let stages: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("ATVPREP-STAGE "))
        .collect();
    assert_eq!(
        stages,
        ["encode", "chunks", "finalize", "verify"],
        "{lines:?}"
    );

    let position = |stage: &str| {
        lines
            .iter()
            .position(|l| l == &format!("ATVPREP-STAGE {stage}"))
            .unwrap()
    };
    let in_chunks: Vec<u32> = lines[position("chunks") + 1..position("finalize")]
        .iter()
        .filter_map(|l| l.strip_prefix("ATVPREP-PROGRESS ")?.parse().ok())
        .collect();
    assert!(
        !in_chunks.is_empty(),
        "no progress in the chunk stage: {lines:?}"
    );
    assert!(in_chunks.windows(2).all(|w| w[0] < w[1]), "{in_chunks:?}");
}

#[test]
fn without_mp4box_a_manager_item_fails_clearly_before_ffmpeg_runs() {
    let mut r = run(false);
    let source = twelve_seconds(&r.base);
    let marker = r.base.join("ffmpeg-ran");
    let fake = r.base.join("fake-ffmpeg");
    atvprep_testkit::write_script(
        &fake,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    );
    r.ctx.ffmpeg = Some(fake);
    let before = sha(&source);

    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();

    assert_eq!(result.error_kind, Some(ErrorKind::Mp4box));
    assert_eq!(result.error_kind.map(ErrorKind::as_str), Some("mp4box"));
    let message = result.error.unwrap();
    assert!(message.contains("MP4Box"), "{message}");
    assert!(
        message.contains("not found") || message.contains("missing"),
        "{message}"
    );
    assert!(!marker.exists(), "ffmpeg must not run without MP4Box");
    assert_eq!(sha(&source), before);
}

#[test]
fn without_mp4box_the_hook_warns_and_delivers_the_file_as_ffmpeg_wrote_it() {
    let mut r = run(false);
    let source = twelve_seconds(&r.base);
    let library = r.base.join("library");
    std::fs::create_dir(&library).unwrap();
    let destination = library.join("Film.mkv");

    let mut stdout = Vec::new();
    let args = [source.clone().into(), destination.into()];
    let code = run_hook(&args, &mut r.ctx, &mut stdout);

    assert_eq!(code, 0, "{}", r.console.contents());
    let delivered = library.join("Film.mp4");
    assert_eq!(
        String::from_utf8(stdout).unwrap(),
        format!("[MediaFile] {}\n", delivered.display())
    );
    assert!(
        r.console.contents().contains("MP4Box"),
        "{}",
        r.console.contents()
    );
    // ffmpeg's chunks of one or two samples are still there: nothing rewrote them.
    let means = mean_chunk_seconds(&delivered);
    assert!(means.iter().all(|(_, mean)| *mean < 0.25), "{means:?}");
}

/// What a file keeps through the chunk stage, as ffprobe reports it.
fn fingerprint(path: &Path) -> Value {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-show_chapters",
        ])
        .args(["-of", "json"])
        .arg(path)
        .output()
        .unwrap();
    let probe: Value = serde_json::from_slice(&output.stdout).unwrap();
    let streams: Vec<Value> = probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            json!({
                "type": s["codec_type"], "codec": s["codec_name"], "profile": s["profile"],
                "channels": s["channels"], "layout": s["channel_layout"],
                "rate": s["sample_rate"], "tags": s["tags"], "disposition": s["disposition"],
            })
        })
        .collect();
    let chapters: Vec<Value> = probe["chapters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({"start": c["start_time"], "end": c["end_time"], "tags": c["tags"]}))
        .collect();
    let mut format_tags = probe["format"]["tags"].clone();
    // ffmpeg's own version string is nothing the stage could lose.
    if let Some(tags) = format_tags.as_object_mut() {
        tags.remove("encoder");
    }
    json!({"streams": streams, "chapters": chapters, "format_tags": format_tags})
}

/// The moov as Bento4 prints it, without sizes, offsets, durations and the chunk tables themselves.
fn moov_outline(path: &Path) -> Vec<String> {
    let dump = SystemTools::new().run_tool("mp4dump", path).unwrap();
    let mut skipping = BTreeSet::new();
    let mut lines = Vec::new();
    for line in dump.lines() {
        let indent = line.len() - line.trim_start().len();
        skipping.retain(|depth| *depth < indent);
        let text = line.trim();
        if text.starts_with("[stco]")
            || text.starts_with("[co64]")
            || text.starts_with("[stsc]")
            || text.starts_with("[free]")
            || text.starts_with("[mdat]")
        {
            skipping.insert(indent);
            continue;
        }
        if !skipping.is_empty() || text.contains("duration") || text.starts_with("compressor") {
            continue;
        }
        let text = text.split(" size=").next().unwrap_or(text);
        lines.push(format!("{}{text}", " ".repeat(indent)));
    }
    lines
}

fn pretty(value: &Value) -> Vec<String> {
    serde_json::to_string_pretty(value)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Both sides as lists of lines; a difference names the lines only one side has.
fn assert_same(name: &str, what: &str, before: &[String], after: &[String]) {
    if before == after {
        return;
    }
    let only = |a: &[String], b: &[String]| {
        a.iter()
            .filter(|l| !b.contains(l))
            .cloned()
            .collect::<Vec<_>>()
    };
    let first = before.iter().zip(after).position(|(a, b)| a != b);
    panic!(
        "{name}: {what} differs\n only before: {:#?}\n only after: {:#?}\n first difference at {first:?}: {:?} / {:?}",
        only(before, after),
        only(after, before),
        first.map(|i| &before[i]),
        first.map(|i| &after[i]),
    );
}

/// The same input twice through Prepare: once as ffmpeg and the moov patch leave it
/// (without `MP4Box`, the hook's fallback), once with the chunk stage. Atmos, tags, languages,
/// subtitles and chapters must not differ.
fn produce_both(name: &str) -> (PathBuf, PathBuf, [Run; 2]) {
    let fixture = atvprep_testkit::fixture_path(name);
    let produce = |present: bool| {
        let mut r = run(present);
        let source = r.base.join(name);
        std::fs::copy(&fixture, &source).unwrap();
        let options = PrepareOptions {
            force: true,
            encode: EncodeAllowance::All,
            output: Some(r.base.join("out.mp4")),
            mp4box_required: false,
            ..PrepareOptions::default()
        };
        let result = prepare_file(&mut r.ctx, &source, &options, None).unwrap();
        assert_eq!(result.error, None, "{name}");
        let out = r.base.join("out.mp4");
        (r, out)
    };
    let (without, plain) = produce(false);
    let (with, chunked) = produce(true);
    (plain, chunked, [without, with])
}

fn assert_the_chunk_stage_changes_nothing_else(name: &str) {
    let (plain, chunked, _runs) = produce_both(name);

    // Known gap, see `short_track_titles_are_cleared_by_mp4box`; MP4Box assigns the es_id anew.
    let comparable = |lines: Vec<String>| -> Vec<String> {
        lines
            .into_iter()
            .filter(|l| !l.contains("\"name\":") && !l.contains("es_id ="))
            .collect()
    };
    assert_same(
        name,
        "ffprobe",
        &comparable(pretty(&fingerprint(&plain))),
        &comparable(pretty(&fingerprint(&chunked))),
    );
    assert_same(
        name,
        "moov",
        &comparable(moov_outline(&plain)),
        &comparable(moov_outline(&chunked)),
    );
}

#[test]
fn atmos_signalling_survives_the_chunk_stage() {
    needs_mp4box!();
    assert_the_chunk_stage_changes_nothing_else("atmos_dec3.mp4");
}

#[test]
fn tags_languages_subtitles_and_chapters_survive_the_chunk_stage() {
    needs_mp4box!();
    assert_the_chunk_stage_changes_nothing_else("chapters.mp4");
    assert_the_chunk_stage_changes_nothing_else("stereo_surround_subs.mp4");
    assert_the_chunk_stage_changes_nothing_else("plain_moov_last.mp4");
}

/// Known gap (ticket #143, open): `MP4Box` reads an ffmpeg file only with `-no-check` when the
/// title of a track is shorter than four characters, and clears these titles. The moov patch does not
/// restore them. The test pins the current state; if it changes, the gap is closed.
#[test]
fn short_track_titles_are_cleared_by_mp4box() {
    needs_mp4box!();
    let (plain, chunked, runs) = produce_both("chapters.mp4");
    let names = |path: &Path| -> Vec<String> {
        pretty(&fingerprint(path))
            .into_iter()
            .filter(|l| l.contains("\"name\":"))
            .map(|l| l.trim().to_owned())
            .collect()
    };
    assert!(names(&plain).iter().any(|n| n == "\"name\": \"DEU\""));
    assert!(names(&chunked).iter().all(|n| n != "\"name\": \"DEU\""));
    // The stage says so in the log.
    assert!(
        runs[1].console.contents().contains("-no-check"),
        "{}",
        runs[1].console.contents()
    );
}

/// Twelve seconds of H.264 and AAC stereo at 48 kHz as ffmpeg writes them with default settings
/// (chunks of one or two samples). Otherwise ready for Apple TV: the only defect is the
/// chunk length. `audio_title` sets a track title (the rest stays the same).
fn only_short_chunks(dir: &Path, audio_title: Option<&str>) -> PathBuf {
    let path = dir.join("Film.mp4");
    let raw = dir.join("Raw.mp4");
    let title = audio_title.map(|t| format!("title={t}"));
    let mut args = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000",
        "-t",
        "12",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-profile:v",
        "high",
        "-c:a",
        "aac",
        "-ac",
        "2",
        "-metadata:s:v:0",
        "language=deu",
        "-metadata:s:a:0",
        "language=deu",
        "-disposition:a:0",
        "default",
        "-movflags",
        "+faststart",
    ];
    if let Some(title) = &title {
        args.extend(["-metadata:s:a:0", title]);
    }
    args.push(raw.to_str().unwrap());
    ffmpeg(&args);
    // Like an existing target file from Prepare without the chunk stage: ffmpeg and moov patch (tags,
    // elng), chunks of one or two samples.
    let mut before_mp4box = run(false);
    let options = PrepareOptions {
        output: Some(path.clone()),
        mp4box_required: false,
        ..all_audio()
    };
    let made = prepare_file(&mut before_mp4box.ctx, &raw, &options, None).unwrap();
    assert_eq!(made.error, None);
    std::fs::remove_file(&raw).unwrap();
    path
}

/// An ffmpeg that must not run: it leaves a marker and fails.
fn forbid_ffmpeg(r: &mut Run) -> PathBuf {
    let marker = r.base.join("ffmpeg-ran");
    let fake = r.base.join("fake-ffmpeg");
    atvprep_testkit::write_script(
        &fake,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    );
    r.ctx.ffmpeg = Some(fake);
    marker
}

fn name_lines(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .filter(|l| !l.contains("\"name\":") && !l.contains("es_id ="))
        .collect()
}

#[test]
fn a_file_with_only_short_chunks_is_rewritten_by_mp4box_alone() {
    needs_mp4box!();
    let mut r = run(true);
    let source = only_short_chunks(&r.base, None);
    let marker = forbid_ffmpeg(&mut r);
    // MP4Box orders children of the same box differently (stss/ctts); the content counts, not the order.
    let sorted = |mut lines: Vec<String>| {
        lines.sort();
        lines
    };
    let before = (
        name_lines(pretty(&fingerprint(&source))),
        sorted(name_lines(moov_outline(&source))),
    );
    let means = mean_chunk_seconds(&source);
    assert!(means.iter().all(|(_, mean)| *mean < 0.25), "{means:?}");

    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();

    assert_eq!(result.error, None, "{}", r.console.contents());
    assert!(result.ok());
    assert_eq!(result.outcome, Outcome::Rebuilt);
    assert!(!marker.exists(), "ffmpeg must not run for the chunks alone");
    for (kind, mean) in mean_chunk_seconds(&source) {
        assert!((0.4..=0.6).contains(&mean), "{kind}: mean {mean:.3} s");
    }
    // Streams, tags, dispositions and the moov stay as they were.
    assert_same(
        "only-chunks",
        "ffprobe",
        &before.0,
        &name_lines(pretty(&fingerprint(&source))),
    );
    assert_same(
        "only-chunks",
        "moov",
        &before.1,
        &sorted(name_lines(moov_outline(&source))),
    );
    // Afterwards Check reports no defect any more, and the original left no leftovers.
    let report = result.report.expect("report");
    assert!(
        report
            .violations
            .iter()
            .all(|v| v.rule != atvprep_core::mp4::chunks::CHUNK_RULE),
        "{:?}",
        report.violations
    );
    let leftovers: Vec<_> = std::fs::read_dir(&r.base)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn the_chunks_only_run_shows_the_chunk_stage_and_no_encode_stage() {
    needs_mp4box!();
    let mut r = run(true);
    let source = only_short_chunks(&r.base, None);
    forbid_ffmpeg(&mut r);
    prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();
    let lines = r.machine.lines();
    let stages: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("ATVPREP-STAGE "))
        .collect();
    assert_eq!(stages, ["chunks", "finalize", "verify"], "{lines:?}");
    let percents: Vec<u32> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("ATVPREP-PROGRESS ")?.parse().ok())
        .collect();
    assert!(percents.windows(2).all(|w| w[0] <= w[1]), "{percents:?}");
    assert!(percents.len() > 3, "{percents:?}");
}

#[test]
fn the_chunks_only_run_needs_no_encode_allowance() {
    needs_mp4box!();
    let mut r = run(true);
    let source = only_short_chunks(&r.base, None);
    let options = PrepareOptions {
        encode: EncodeAllowance::Copy,
        ..PrepareOptions::default()
    };
    let result = prepare_file(&mut r.ctx, &source, &options, None).unwrap();
    assert_eq!(result.outcome, Outcome::Rebuilt, "{result:?}");
    assert!(result.encode_refusal.is_none());
}

#[test]
fn a_chunks_only_run_to_an_output_leaves_the_source_alone() {
    needs_mp4box!();
    let mut r = run(true);
    let source = only_short_chunks(&r.base, None);
    forbid_ffmpeg(&mut r);
    let before = sha(&source);
    let out = r.base.join("out.mp4");
    let options = PrepareOptions {
        output: Some(out.clone()),
        ..all_audio()
    };
    let result = prepare_file(&mut r.ctx, &source, &options, None).unwrap();
    assert_eq!(result.outcome, Outcome::Rebuilt, "{result:?}");
    assert_eq!(sha(&source), before);
    for (kind, mean) in mean_chunk_seconds(&out) {
        assert!((0.4..=0.6).contains(&mean), "{kind}: mean {mean:.3} s");
    }
}

#[test]
fn a_dry_run_of_a_chunks_only_file_writes_nothing() {
    let mut r = run(true);
    let source = only_short_chunks(&r.base, None);
    let before = sha(&source);
    let options = PrepareOptions {
        dry_run: true,
        ..all_audio()
    };
    let result = prepare_file(&mut r.ctx, &source, &options, None).unwrap();
    assert_eq!(result.outcome, Outcome::Planned);
    assert_eq!(sha(&source), before);
}

#[test]
fn without_mp4box_a_chunks_only_item_fails_clearly_and_leaves_the_file() {
    let mut r = run(false);
    let source = only_short_chunks(&r.base, None);
    let marker = forbid_ffmpeg(&mut r);
    let before = sha(&source);
    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();
    assert_eq!(result.error_kind, Some(ErrorKind::Mp4box));
    assert!(result.error.unwrap().contains("MP4Box"));
    assert!(!marker.exists());
    assert_eq!(sha(&source), before);
}

#[test]
fn without_mp4box_the_hook_style_run_warns_and_leaves_a_chunks_only_file() {
    let mut r = run(false);
    let source = only_short_chunks(&r.base, None);
    let marker = forbid_ffmpeg(&mut r);
    let before = sha(&source);
    let options = PrepareOptions {
        mp4box_required: false,
        ..all_audio()
    };
    let result = prepare_file(&mut r.ctx, &source, &options, None).unwrap();
    assert_eq!(result.error, None);
    assert_eq!(result.outcome, Outcome::Done, "{result:?}");
    assert!(r.console.contents().contains("MP4Box"));
    assert!(!marker.exists());
    assert_eq!(sha(&source), before);
}

/// Known gap (ticket #143, open), in the MP4Box-only path (#145): if `MP4Box` reads the file only with
/// `-no-check` because a track title has fewer than four characters, it clears these titles; the moov patch
/// does not restore them. The test pins the current state; if it changes, the gap is closed.
#[test]
fn short_track_titles_are_cleared_by_the_chunks_only_run_too() {
    needs_mp4box!();
    let mut r = run(true);
    let source = only_short_chunks(&r.base, Some("DEU"));
    let titled = |path: &Path| {
        pretty(&fingerprint(path))
            .iter()
            .any(|l| l.contains("\"name\": \"DEU\""))
    };
    assert!(titled(&source));
    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();
    assert_eq!(result.error, None, "{}", r.console.contents());
    assert!(!titled(&source));
    assert!(
        r.console.contents().contains("-no-check"),
        "{}",
        r.console.contents()
    );
}

/// Atmos signalling, tags, languages, subtitles and chapters stay in the MP4Box-only path.
fn assert_chunks_only_changes_nothing_else(name: &str) {
    needs_mp4box!();
    // The file as Prepare wrote it without the chunk stage: ffmpeg, moov patch, short chunks.
    let (plain, _chunked, _runs) = produce_both(name);
    let mut r = run(true);
    let source = r.base.join("Film.mp4");
    std::fs::copy(&plain, &source).unwrap();
    let marker = forbid_ffmpeg(&mut r);
    let sorted = |mut lines: Vec<String>| {
        lines.sort();
        lines
    };
    let before = (
        name_lines(pretty(&fingerprint(&source))),
        sorted(name_lines(moov_outline(&source))),
    );
    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();
    assert_eq!(result.error, None, "{name}: {}", r.console.contents());
    assert_eq!(result.outcome, Outcome::Rebuilt, "{name}");
    assert!(!marker.exists(), "{name}: ffmpeg ran");
    assert_same(
        name,
        "ffprobe",
        &before.0,
        &name_lines(pretty(&fingerprint(&source))),
    );
    assert_same(
        name,
        "moov",
        &before.1,
        &sorted(name_lines(moov_outline(&source))),
    );
}

#[test]
fn atmos_and_tags_survive_the_chunks_only_run() {
    assert_chunks_only_changes_nothing_else("atmos_dec3.mp4");
    assert_chunks_only_changes_nothing_else("chapters.mp4");
    assert_chunks_only_changes_nothing_else("stereo_surround_subs.mp4");
}

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
use atvprep_core::mp4::patch::restore_from;
use atvprep_core::pipeline_postfix::RunContext;
use atvprep_core::prepare::chunks::{Check, Mp4Box};
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

/// A file as rich as a feature film: video, four audio tracks, two subtitle tracks (a few sparse
/// samples each) and chapters, written by ffmpeg with default settings.
fn many_tracks(dir: &Path) -> PathBuf {
    let srt = dir.join("subs.srt");
    std::fs::write(
        &srt,
        "1\n00:00:01,000 --> 00:00:03,000\nHello\n\n2\n00:00:09,000 --> 00:00:11,000\nWorld\n\n3\n00:00:17,000 --> 00:00:19,000\nEnd\n",
    )
    .unwrap();
    let chapters = dir.join("chapters.txt");
    std::fs::write(
        &chapters,
        ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=8000\ntitle=One\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=8000\nEND=20000\ntitle=Two\n",
    )
    .unwrap();
    let path = dir.join("Many.mp4");
    let mut args: Vec<String> = Vec::new();
    let mut add = |items: &[&str]| args.extend(items.iter().map(|s| (*s).to_owned()));
    add(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=24:duration=20",
    ]);
    for frequency in [440, 660, 880, 220] {
        add(&[
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency={frequency}:sample_rate=48000:duration=20"),
        ]);
    }
    add(&["-i", srt.to_str().unwrap(), "-i", srt.to_str().unwrap()]);
    add(&["-i", chapters.to_str().unwrap()]);
    add(&[
        "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:a", "-map", "4:a",
    ]);
    add(&["-map", "5", "-map", "6", "-map_chapters", "7"]);
    add(&[
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-c:s", "mov_text",
    ]);
    for (index, language) in ["deu", "eng", "fra", "spa"].iter().enumerate() {
        add(&[
            &format!("-metadata:s:a:{index}"),
            &format!("language={language}"),
        ]);
    }
    for (index, language) in ["deu", "eng"].iter().enumerate() {
        add(&[
            &format!("-metadata:s:s:{index}"),
            &format!("language={language}"),
        ]);
    }
    add(&[path.to_str().unwrap()]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ffmpeg(&refs);
    path
}

/// `MP4Box -inter` corrects drift between the tracks; a sparse subtitle track made it cut the chunks of
/// every other track to a fraction of the duration asked for (0.13 s with seven tracks).
#[test]
fn every_audio_and_video_track_of_a_many_track_file_has_chunks_of_half_a_second() {
    needs_mp4box!();
    let mut r = run(true);
    let source = many_tracks(&r.base);
    let before = mean_chunk_seconds(&source);
    assert_eq!(before.len(), 5, "{before:?}");
    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();
    assert_eq!(result.error, None);

    let means = mean_chunk_seconds(&source);
    // Prepare keeps the video and the audio tracks of the languages it supports (here two).
    assert_eq!(means.len(), 3, "{means:?}");
    for (kind, mean) in &means {
        assert!(
            (0.4..=0.6).contains(mean),
            "{kind}: mean chunk duration {mean:.3} s of {means:?}"
        );
    }
    // The check that flags a file for the chunk length no longer finds anything.
    assert_eq!(
        atvprep_core::mp4::chunks::short_chunks(&source).unwrap(),
        []
    );
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

/// The track titles as the `udta/name` atoms of the tracks hold them, as `"name": "DEU"` lines. Read from the
/// moov itself: which tag key ffprobe shows them under (or whether at all) depends on its version.
fn title_lines(path: &Path) -> Vec<String> {
    atoms(path)
        .into_iter()
        .filter_map(|line| {
            let hex = line.split_once(" trak/udta/name ")?.1;
            let text: String = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap_or(b'?'))
                .filter(|b| b.is_ascii_graphic() || *b == b' ')
                .map(char::from)
                .collect();
            Some(format!("\"name\": \"{text}\""))
        })
        .collect()
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

/// Every leaf atom of the moov as "path payload-hex", per track and sorted: the sample tables (chunk
/// offsets, sample-to-chunk, timing, sizes, sync samples) are the only atoms `MP4Box` may write differently.
fn atoms(path: &Path) -> Vec<String> {
    atoms_of(path, false)
}

/// [`atoms`] without the `alternate_group` of every `tkhd`: the moov patch sets it (audio 1,
/// subtitles 2), and not every route through Prepare runs it.
fn atoms_ignoring_groups(path: &Path) -> Vec<String> {
    atoms_of(path, true)
}

fn atoms_of(path: &Path, ignore_groups: bool) -> Vec<String> {
    use atvprep_core::mp4::boxes::iter_boxes;
    fn walk(
        data: &[u8],
        start: usize,
        end: usize,
        path: &str,
        ignore_groups: bool,
        lines: &mut Vec<String>,
    ) {
        for b in iter_boxes(data, start, end) {
            let kind = String::from_utf8_lossy(&b.kind).into_owned();
            let here = format!("{path}/{kind}");
            match &b.kind {
                b"mdia" | b"minf" | b"stbl" | b"edts" | b"udta" | b"dinf" | b"tref" | b"ilst" => {
                    walk(data, b.start + 8, b.end, &here, ignore_groups, lines);
                }
                b"meta" => walk(data, b.start + 12, b.end, &here, ignore_groups, lines),
                b"stsd" => walk(data, b.start + 16, b.end, &here, ignore_groups, lines),
                b"stco" | b"co64" | b"stsc" | b"stts" | b"stsz" | b"stz2" | b"stss" | b"ctts"
                | b"sdtp" | b"cslg" | b"sgpd" | b"sbgp" | b"free" => {}
                _ => {
                    let mut payload = data[b.start + 8..b.end].to_vec();
                    // MP4Box rounds the durations of the moov by a tick on some platforms: the sample
                    // tables, which decide them, are left out of this comparison anyway.
                    let wide = payload.first() == Some(&1);
                    let duration = match &b.kind {
                        b"mvhd" | b"mdhd" => Some(if wide { 24..32 } else { 16..20 }),
                        b"tkhd" => Some(if wide { 28..36 } else { 20..24 }),
                        _ => None,
                    };
                    if let Some(range) = duration {
                        payload[range].fill(0);
                    }
                    if ignore_groups && &b.kind == b"tkhd" {
                        let at = if payload.first() == Some(&1) { 46 } else { 34 };
                        payload[at..at + 2].fill(0);
                    }
                    lines.push(format!("{here} {}", atvprep_testkit::hex_encode(&payload)));
                }
            }
        }
    }
    let moov = atvprep_core::mp4::patch::read_moov_of(path)
        .unwrap()
        .expect("a moov");
    let mut lines = Vec::new();
    let mut tracks = 0;
    for b in iter_boxes(&moov, 8, moov.len()) {
        if &b.kind == b"trak" {
            let mut track = Vec::new();
            walk(&moov, b.start + 8, b.end, "trak", ignore_groups, &mut track);
            track.sort();
            lines.extend(track.into_iter().map(|l| format!("#{tracks} {l}")));
            tracks += 1;
        } else {
            let mut other = Vec::new();
            walk(&moov, b.start, b.end, "", ignore_groups, &mut other);
            lines.extend(other.into_iter().map(|l| format!("moov{l}")));
        }
    }
    lines
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
    // Track titles ("name"), es_id and everything else the moov holds must be there as well.
    assert_same(
        name,
        "ffprobe",
        &pretty(&fingerprint(&plain)),
        &pretty(&fingerprint(&chunked)),
    );
    assert_same(name, "moov", &moov_outline(&plain), &moov_outline(&chunked));
    assert_same(
        name,
        "atoms",
        &atoms_ignoring_groups(&plain),
        &atoms_ignoring_groups(&chunked),
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

/// `MP4Box` reads an ffmpeg file only with `-no-check` when the title of a track is shorter than four
/// characters, and then clears these titles. The restore step after the run puts them back (ADR 0010).
#[test]
fn short_track_titles_survive_mp4box() {
    needs_mp4box!();
    let (plain, chunked, runs) = produce_both("chapters.mp4");
    let names = |path: &Path| title_lines(path);
    assert_eq!(names(&plain), names(&chunked));
    // Newer ffmpeg versions drop a title that equals the language of the track, so the fixture
    // only has a short title to lose with some of them; then the fallback ran, and the stage says
    // what it put back. `mp4box_loses_nothing_of_a_rich_ffmpeg_file` makes its own short titles.
    if names(&plain).iter().any(|n| n == "\"name\": \"DEU\"") {
        let log = runs[1].console.contents();
        assert!(log.contains("-no-check"), "{log}");
        assert!(log.contains("restored"), "{log}");
    }
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
    let before = (pretty(&fingerprint(&source)), sorted(moov_outline(&source)));
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
        &pretty(&fingerprint(&source)),
    );
    assert_same(
        "only-chunks",
        "moov",
        &before.1,
        &sorted(moov_outline(&source)),
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

/// In the MP4Box-only path (#145), too: `-no-check` clears titles of fewer than four characters, the restore
/// step after the run puts them back.
#[test]
fn short_track_titles_survive_the_chunks_only_run() {
    needs_mp4box!();
    let mut r = run(true);
    let source = only_short_chunks(&r.base, Some("Dub"));
    let titled = |path: &Path| title_lines(path).iter().any(|l| l == "\"name\": \"Dub\"");
    assert!(titled(&source));
    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();
    assert_eq!(result.error, None, "{}", r.console.contents());
    assert!(titled(&source));
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
    let before = (pretty(&fingerprint(&source)), sorted(moov_outline(&source)));
    let atoms_before = atoms_ignoring_groups(&source);
    let result = prepare_file(&mut r.ctx, &source, &all_audio(), None).unwrap();
    assert_eq!(result.error, None, "{name}: {}", r.console.contents());
    assert_same(
        name,
        "atoms",
        &atoms_before,
        &atoms_ignoring_groups(&source),
    );
    assert_eq!(result.outcome, Outcome::Rebuilt, "{name}");
    assert!(!marker.exists(), "{name}: ffmpeg ran");
    assert_same(name, "ffprobe", &before.0, &pretty(&fingerprint(&source)));
    assert_same(name, "moov", &before.1, &sorted(moov_outline(&source)));
}

#[test]
fn atmos_and_tags_survive_the_chunks_only_run() {
    assert_chunks_only_changes_nothing_else("atmos_dec3.mp4");
    assert_chunks_only_changes_nothing_else("chapters.mp4");
    assert_chunks_only_changes_nothing_else("stereo_surround_subs.mp4");
}

/// Fixtures that carry streams: every moov atom except the sample tables must come out of `MP4Box`
/// as it went in (ADR 0010). Titles, `dec3`, `esds`, handler names, languages, `tkhd` and `mdhd`.
#[test]
fn mp4box_loses_no_atom_of_the_moov_in_the_fixtures() {
    needs_mp4box!();
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let mut fixtures = Vec::new();
    for dir in ["mp4", "prepare"] {
        for entry in std::fs::read_dir(testdata.join(dir)).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.extension().is_some_and(|e| e == "mp4") && !name.starts_with("syn_") {
                fixtures.push((format!("{dir}-{name}"), path));
            }
        }
    }
    fixtures.sort();
    assert!(fixtures.len() >= 10, "{fixtures:?}");
    let dir = tempfile::tempdir().unwrap();
    for (name, fixture) in fixtures {
        let input = dir.path().join(format!("in-{name}"));
        let output = dir.path().join(format!("out-{name}"));
        std::fs::copy(&fixture, &input).unwrap();
        let done = Mp4Box::new().rechunk(&input, &output, &mut |_| {}).unwrap();
        restore_from(&output, &done.reference.unwrap()).unwrap();
        assert_same(&name, "atoms", &atoms(&input), &atoms(&output));
    }
}

/// A fresh ffmpeg file with everything Apple TV files carry: track titles of three characters and
/// longer, languages, dispositions, chapters, tags, AC-3 5.1, `mov_text` subtitles.
fn rich_ffmpeg_file(dir: &Path) -> PathBuf {
    let meta = dir.join("meta.txt");
    std::fs::write(
        &meta,
        ";FFMETADATA1\ntitle=Film\nartist=Someone\ncomment=hello\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=6000\ntitle=One\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=6000\nEND=12000\ntitle=Two\n",
    )
    .unwrap();
    let srt = dir.join("s.srt");
    std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,000\nHallo\n").unwrap();
    let path = dir.join("rich.mp4");
    let (meta, srt, out) = (
        meta.to_str().unwrap(),
        srt.to_str().unwrap(),
        path.to_str().unwrap(),
    );
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:sample_rate=48000",
        "-i",
        srt,
        "-i",
        srt,
        "-i",
        meta,
        "-t",
        "12",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:a",
        "-map",
        "3",
        "-map",
        "4",
        "-map_metadata",
        "5",
        "-map_chapters",
        "5",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "ac3",
        "-c:s",
        "mov_text",
        "-metadata:s:v:0",
        "title=Video",
        "-metadata:s:a:0",
        "title=Dub",
        "-metadata:s:a:0",
        "language=deu",
        "-metadata:s:a:1",
        "title=English Stereo",
        "-metadata:s:a:1",
        "language=eng",
        "-metadata:s:s:0",
        "title=Org",
        "-metadata:s:s:0",
        "language=eng",
        "-metadata:s:s:1",
        "title=Deutsch SDH",
        "-metadata:s:s:1",
        "language=deu",
        "-disposition:a:0",
        "default",
        "-disposition:a:1",
        "0",
        "-disposition:s:0",
        "0",
        "-disposition:s:1",
        "+hearing_impaired+forced",
        "-movflags",
        "+faststart",
        out,
    ]);
    path
}

#[test]
fn mp4box_loses_nothing_of_a_rich_ffmpeg_file() {
    needs_mp4box!();
    let dir = tempfile::tempdir().unwrap();
    let input = rich_ffmpeg_file(dir.path());
    let output = dir.path().join("out.mp4");
    let done = Mp4Box::new().rechunk(&input, &output, &mut |_| {}).unwrap();
    // Titles of three characters are what makes MP4Box reject the file.
    assert_eq!(done.check, Check::Skip);
    let restored = restore_from(&output, &done.reference.unwrap()).unwrap();
    assert!(!restored.is_empty());
    assert_same("rich", "atoms", &atoms(&input), &atoms(&output));
    assert_same(
        "rich",
        "ffprobe",
        &pretty(&fingerprint(&input)),
        &pretty(&fingerprint(&output)),
    );
    let titles = title_lines(&output);
    assert!(
        titles.iter().any(|t| t == "\"name\": \"Dub\""),
        "{titles:?}"
    );
    assert!(
        titles.iter().any(|t| t == "\"name\": \"Org\""),
        "{titles:?}"
    );
}

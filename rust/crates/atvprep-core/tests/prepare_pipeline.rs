//! `prepare_file` against the Python oracle: the same sources and option sets, the same
//! observable results (result fields, directory, untouched bytes, stream signatures of what ffmpeg
//! wrote, the ffmpeg argv, the machine lines).
//!
//! Needs ffmpeg, ffprobe and Bento4 on `PATH`, like `real_tools.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use atvprep_core::compliance::SystemTools;
use atvprep_core::console::{Console, SharedBuffer};
use atvprep_core::encode::{EncodeAllowance, PlanStep};
use atvprep_core::fd_fix_contract::FdFixTarget;
use atvprep_core::ffprobe::Ffprobe;
use atvprep_core::pipeline_postfix::RunContext;
use atvprep_core::prepare::pipeline::{PrepareError, planned_change, prepare_file};
use atvprep_core::prepare::{ErrorKind, PrepareOptions, PrepareResult};
use atvprep_core::stages::StageReporter;
use atvprep_core::verify::verify_file;
use atvprep_testkit::{load_golden, sha256_hex};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

static TEMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\.tmp_fixmeta_.*)\.\d+-[0-9a-f]{8}\.mp4").expect("valid"));

static PROGRESS_FD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"-progress","/dev/fd/\d+"#).expect("valid"));

#[derive(Deserialize)]
struct Input {
    source: String,
    variant: String,
}

fn sources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/prepare")
}

fn normalise(text: &str, base: &Path) -> String {
    let resolved = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let text = text
        .replace(&resolved.to_string_lossy().into_owned(), "<dir>")
        .replace(&base.to_string_lossy().into_owned(), "<dir>");
    TEMP.replace_all(&text, "$1.<pid>.mp4").into_owned()
}

fn rel_or_null(path: Option<&Path>, base: &Path) -> Value {
    path.map_or(Value::Null, |p| {
        json!(normalise(&p.to_string_lossy(), base).replace("<dir>/", ""))
    })
}

fn copy_case(name: &str, work: &Path) -> PathBuf {
    std::fs::create_dir_all(work).unwrap();
    let put = |from: &str, to: &str| {
        std::fs::copy(sources().join(from), work.join(to)).unwrap();
    };
    if name == "sibling.mkv" {
        put("movie.mkv", name);
        put("clean.mp4", "sibling.mp4");
    } else {
        put(name, name);
        if name == "movie.mkv" {
            put("movie.nl.srt", "movie.nl.srt");
        }
    }
    work.join(name)
}

fn options_for(variant: &str, base: &Path, source: &Path) -> PrepareOptions {
    let out = base.join("out");
    let mut o = PrepareOptions::default();
    match variant {
        "force" => o.force = true,
        "dry_run" => o.dry_run = true,
        // The oracle encoded the video on these routes; since ADR 0008 that takes an allowance.
        "delete_original" => {
            o.delete_original = true;
            o.encode = EncodeAllowance::All;
        }
        "copy_only" => o.encode = EncodeAllowance::Copy,
        "output_file" => {
            o.output = Some(out.join("result.mp4"));
            o.encode = EncodeAllowance::All;
        }
        "output_dir" => {
            o.output = Some(out);
            o.encode = EncodeAllowance::All;
        }
        "output_new_dir" => {
            o.output = Some(base.join("new").join("sub"));
            o.encode = EncodeAllowance::All;
        }
        "output_self" => o.output = Some(source.to_path_buf()),
        "quarantine" | "quarantine_target" => o.quarantine_dir = Some(base.join("q")),
        "delete_with_output" => {
            o.delete_original = true;
            o.output = Some(out);
        }
        _ => {}
    }
    o
}

/// A script that records its argv like the oracle's shim, then runs the real ffmpeg.
fn shim(dir: &Path) -> (PathBuf, PathBuf) {
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("ffmpeg"))
        .find(|p| p.is_file())
        .expect("ffmpeg on PATH");
    let log = dir.join("ffmpeg.log");
    let script = dir.join("ffmpeg");
    let body = format!(
        "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\0' \"$arg\" >> '{log}'; done\nprintf '\\0' >> '{log}'\nexec '{real}' \"$@\"\n",
        log = log.display(),
        real = real.display()
    );
    atvprep_testkit::write_script(&script, body);
    (script, log)
}

fn argv_runs(log: &Path) -> Vec<Vec<String>> {
    let Ok(bytes) = std::fs::read(log) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    let mut current = Vec::new();
    let parts: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    for part in &parts[..parts.len() - 1] {
        if part.is_empty() {
            runs.push(std::mem::take(&mut current));
        } else {
            current.push(String::from_utf8_lossy(part).into_owned());
        }
    }
    runs
}

fn files_under(base: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(base, &mut out);
    out.sort();
    out
}

fn sha(path: &Path) -> String {
    sha256_hex(&std::fs::read(path).unwrap())
}

fn signature(path: &Path, ffprobe: &Ffprobe) -> Value {
    let probe = ffprobe.probe_json(path).unwrap();
    let streams = probe["streams"].as_array().unwrap();
    Value::Array(
        streams
            .iter()
            .map(|s| {
                json!([
                    s["codec_type"],
                    s["codec_name"],
                    s["tags"]["language"].as_str().unwrap_or(""),
                    s["channels"].as_i64().unwrap_or(0),
                ])
            })
            .collect(),
    )
}

fn severities(path: &Path) -> Value {
    let report = verify_file(path, &SystemTools::new(), None, None);
    let mut pairs: Vec<Value> = report
        .violations
        .iter()
        .map(|v| json!([v.rule, v.severity.as_str()]))
        .collect();
    pairs.sort_by_key(ToString::to_string);
    Value::Array(pairs)
}

fn result_json(result: &PrepareResult, base: &Path) -> Value {
    let mut report: Option<Vec<Value>> = result.report.as_ref().map(|r| {
        r.violations
            .iter()
            .map(|v| json!([v.rule, v.severity.as_str()]))
            .collect()
    });
    if let Some(pairs) = report.as_mut() {
        pairs.sort_by_key(ToString::to_string);
    }
    json!({
        "source": rel_or_null(Some(&result.source), base),
        "output": rel_or_null(result.output.as_deref(), base),
        "level": result.level.as_str(),
        "outcome": result.outcome.as_str(),
        "reasons": result.reasons,
        "tags_only": result.tags_only,
        "source_removed": result.source_removed,
        "source_kept": result.source_kept,
        "error": result.error.as_deref().map(|e| normalise(e, base)),
        "error_kind": result.error_kind.map(ErrorKind::as_str),
        "report": report,
        "report_path": rel_or_null(result.report.as_ref().map(|r| r.path.as_path()), base),
    })
}

fn run_case(name: &str, variant: &str) -> Value {
    let tools = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let source = copy_case(name, &base.join("work"));
    std::fs::create_dir(base.join("out")).unwrap();
    let options = options_for(variant, &base, &source);
    let before: BTreeMap<PathBuf, String> = files_under(&base)
        .into_iter()
        .map(|p| (p.clone(), sha(&p)))
        .collect();
    let (ffmpeg, log) = shim(tools.path());
    let machine = SharedBuffer::default();
    let mut ctx = RunContext {
        console: Console::recording(false).0,
        reporter: StageReporter::new(Box::new(machine.clone()), false),
        tools: Box::new(SystemTools::new()),
        ffprobe: Ffprobe::new(),
        ffmpeg: Some(ffmpeg),
    };
    let target = matches!(variant, "target" | "quarantine_target")
        .then(|| FdFixTarget::for_path(&source).unwrap());
    let (result, raised) = match prepare_file(&mut ctx, &source, &options, target.as_ref()) {
        Ok(result) => (Some(result), Value::Null),
        Err(PrepareError::Usage(message)) => (None, json!(format!("ValueError: {message}"))),
        Err(other) => panic!("{name} {variant}: {other}"),
    };
    drop(target);
    let runs = argv_runs(&log);
    let all = files_under(&base);
    let listing: Vec<String> = all
        .iter()
        .map(|p| normalise(&p.strip_prefix(&base).unwrap().to_string_lossy(), &base))
        .collect();
    let mut files = serde_json::Map::new();
    for path in &all {
        let key = normalise(&path.strip_prefix(&base).unwrap().to_string_lossy(), &base);
        let changed = before.get(path).is_none_or(|old| *old != sha(path));
        let mp4 = path.extension().is_some_and(|e| e == "mp4");
        let value = if !runs.is_empty() && changed && mp4 {
            json!({"signature": signature(path, &ctx.ffprobe), "severities": severities(path)})
        } else {
            json!(sha(path))
        };
        files.insert(key, value);
    }
    let argv: Vec<Vec<String>> = runs
        .iter()
        .map(|run| run.iter().map(|a| normalise(a, &base)).collect())
        .collect();
    let machine: Vec<String> = machine
        .lines()
        .into_iter()
        .filter(|l| l.starts_with("ATVPREP-"))
        .map(|l| normalise(&l, &base))
        .collect();
    let mut expected = json!({
        "raised": raised, "listing": listing, "files": files, "argv": argv, "machine": machine,
    });
    if let Some(result) = result {
        expected["result"] = result_json(&result, &base);
    }
    expected
}

// The progress weights are the port's own since #133 (measured per variant), so the oracle's
// percentages are not compared; its stage lines are. Independently reject out-of-range or
// non-monotonic progress. Plan lines are left out.
fn stable_machine(value: Value) -> Value {
    let Some(lines) = value.as_array() else {
        return value;
    };
    let mut result = Vec::new();
    let mut previous = None;
    for line in lines {
        let text = line.as_str().expect("machine protocol string");
        // The plan lines are the port's own, for the manager's log; the oracle has none.
        if text.starts_with("ATVPREP-PLAN ") {
            continue;
        }
        if let Some(percent) = text.strip_prefix("ATVPREP-PROGRESS ") {
            let percent: u32 = percent.parse().expect("integer progress");
            assert!(percent <= 99, "progress must remain below completion");
            assert!(
                previous.is_none_or(|last| percent > last),
                "progress must increase"
            );
            previous = Some(percent);
            continue;
        }
        result.push(line.clone());
    }
    Value::Array(result)
}

#[test]
fn oracle_progress_is_checked_for_order_but_not_compared() {
    let sampled = json!([
        "ATVPREP-STAGE encode",
        "ATVPREP-PROGRESS 69",
        "ATVPREP-STAGE finalize",
        "ATVPREP-PROGRESS 90",
        "ATVPREP-PROGRESS 97"
    ]);
    let expected = json!(["ATVPREP-STAGE encode", "ATVPREP-STAGE finalize"]);
    assert_eq!(stable_machine(sampled), expected);
    assert_eq!(stable_machine(expected.clone()), expected);
}

#[test]
#[should_panic(expected = "progress must increase")]
fn oracle_progress_that_regresses_is_still_rejected() {
    stable_machine(json!([
        "ATVPREP-STAGE encode",
        "ATVPREP-PROGRESS 86",
        "ATVPREP-PROGRESS 69"
    ]));
}

/// The progress percents a run of `name` with `variant` reported, in order.
fn percents(name: &str, variant: &str) -> Vec<u32> {
    run_case(name, variant)["machine"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str()?.strip_prefix("ATVPREP-PROGRESS "))
        .map(|n| n.parse().unwrap())
        .collect()
}

#[test]
fn a_repack_reports_the_phase_boundaries_55_98_and_99() {
    assert_eq!(percents("movie.mkv", "default"), [55, 98, 99]);
    assert_eq!(percents("moovlast.mp4", "default"), [55, 98, 99]);
}

#[test]
fn a_patch_without_ffmpeg_copies_to_20_patches_to_98_and_rises_to_99() {
    let values = percents("tags.mp4", "default");
    assert!(values.is_sorted(), "{values:?}");
    assert_eq!(values.first(), Some(&20), "{values:?}");
    assert!(values.contains(&98), "{values:?}");
    assert_eq!(values.last(), Some(&99));
    assert!(values.iter().all(|p| *p <= 99));
}

/// The two oracle results ADR 0008 changed on purpose: a video that may not be encoded was
/// `reencode_needed`, or a `transcode` failure under `copy_only`; now both are one refusal.
fn refused_since_adr_0008(mut result: Value) -> Value {
    let refused = result["outcome"] == "reencode_needed"
        || (result["outcome"] == "failed" && result["error_kind"] == "transcode");
    if refused {
        result["outcome"] = json!("encode_not_allowed");
        result["error"] = Value::Null;
        result["error_kind"] = Value::Null;
    }
    result
}

#[test]
fn pipeline_matches_the_python_oracle() {
    let cases = load_golden::<Input, Value>("prepare_pipeline");
    assert_eq!(cases.len(), 91);
    let mut mismatches = Vec::new();
    for case in cases {
        let got = run_case(&case.input.source, &case.input.variant);
        for key in ["raised", "result", "listing", "files", "argv", "machine"] {
            let mut want = case.expected.get(key).cloned().unwrap_or(Value::Null);
            if key == "result" {
                want = refused_since_adr_0008(want);
            }
            if key == "argv" {
                // Python hands ffmpeg a private pipe as /dev/fd/N; the port reads `pipe:1`.
                want = Value::String(
                    PROGRESS_FD
                        .replace_all(&want.to_string(), "-progress\",\"pipe:1")
                        .into_owned(),
                );
            }
            let mut have = got.get(key).cloned().unwrap_or(Value::Null);
            if key == "argv" {
                have = Value::String(have.to_string());
            }
            if key == "machine" {
                want = stable_machine(want);
                have = stable_machine(have);
            }
            if want != have {
                mismatches.push(format!(
                    "{} / {key}:\n  want {want}\n  have {have}",
                    case.name
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// A run of `prepare_file` on a fresh copy of `name` with `ffmpeg` replaced by `script`.
struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    source: PathBuf,
    ctx: RunContext,
}

fn fixture(name: &str, script: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let source = copy_case(name, &base.join("work"));
    let ffmpeg = script.map(|body| {
        let path = base.join("fake-ffmpeg");
        atvprep_testkit::write_script(&path, format!("#!/bin/sh\n{body}\n"));
        path
    });
    let ctx = RunContext {
        console: Console::recording(false).0,
        reporter: StageReporter::new(Box::new(SharedBuffer::default()), false),
        tools: Box::new(SystemTools::new()),
        ffprobe: Ffprobe::new(),
        ffmpeg,
    };
    Fixture {
        _dir: dir,
        base,
        source,
        ctx,
    }
}

fn leftovers(dir: &Path) -> Vec<String> {
    files_under(dir)
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
        .filter(|n| n.starts_with(".tmp_fixmeta_"))
        .collect()
}

#[test]
fn ffmpeg_failure_leaves_no_temp_file() {
    let mut f = fixture("moovlast.mp4", Some("exit 1"));
    let before = sha(&f.source);
    let result = prepare_file(&mut f.ctx, &f.source, &PrepareOptions::default(), None).unwrap();
    assert_eq!(result.error_kind.map(ErrorKind::as_str), Some("ffmpeg"));
    assert!(
        result
            .error
            .unwrap()
            .ends_with("returned non-zero exit status 1.")
    );
    assert_eq!(leftovers(&f.base), Vec::<String>::new());
    assert_eq!(sha(&f.source), before);
}

#[test]
fn no_output_is_a_failure() {
    let mut f = fixture("moovlast.mp4", Some("exit 0"));
    let result = prepare_file(&mut f.ctx, &f.source, &PrepareOptions::default(), None).unwrap();
    assert_eq!(result.error_kind.map(ErrorKind::as_str), Some("no_output"));
    assert_eq!(
        result.error.as_deref(),
        Some("ffmpeg wrote no output for moovlast.mp4")
    );
    assert_eq!(leftovers(&f.base), Vec::<String>::new());
}

#[test]
fn an_output_directory_that_cannot_be_created_fails_as_unsafe() {
    let mut f = fixture("clean.mp4", None);
    let blocker = f.base.join("a-file");
    std::fs::write(&blocker, "x").unwrap();
    let options = PrepareOptions {
        output: Some(blocker.join("sub").join("out.mp4")),
        ..PrepareOptions::default()
    };
    let result = prepare_file(&mut f.ctx, &f.source, &options, None).unwrap();
    assert_eq!(result.error_kind.map(ErrorKind::as_str), Some("unsafe"));
    assert!(
        result
            .error
            .unwrap()
            .starts_with("could not create the output directory:")
    );
    assert_eq!(leftovers(&f.base), Vec::<String>::new());
}

/// ffmpeg that "writes" a fragmented MP4: it passes the patch steps but not the compliance audit.
fn fragmented_ffmpeg() -> String {
    let fixture = atvprep_testkit::fixture_path("syn_fragmented.mp4");
    format!("for last; do :; done\ncp '{}' \"$last\"", fixture.display())
}

#[test]
fn a_re_encoded_mp4_replaces_the_source_only_after_passing_verification() {
    let script = fragmented_ffmpeg();
    let mut f = fixture("high10.mp4", Some(&script));
    let before = sha(&f.source);
    let options = PrepareOptions {
        encode: EncodeAllowance::All,
        ..PrepareOptions::default()
    };
    let result = prepare_file(&mut f.ctx, &f.source, &options, None).unwrap();
    assert_eq!(result.error_kind.map(ErrorKind::as_str), Some("blockers"));
    assert_eq!(sha(&f.source), before);
    assert_eq!(leftovers(&f.base), Vec::<String>::new());
}

#[test]
fn an_original_is_deleted_only_after_its_mp4_passed_verification() {
    let script = fragmented_ffmpeg();
    let mut f = fixture("movie.mkv", Some(&script));
    let before = sha(&f.source);
    let options = PrepareOptions {
        delete_original: true,
        ..PrepareOptions::default()
    };
    let result = prepare_file(&mut f.ctx, &f.source, &options, None).unwrap();
    assert_eq!(result.error_kind.map(ErrorKind::as_str), Some("blockers"));
    assert!(!result.source_removed);
    assert_eq!(sha(&f.source), before);
}

#[test]
fn blockers_are_quarantined_only_for_standalone_runs() {
    let script = fragmented_ffmpeg();
    // Standalone: the failing output goes to the quarantine directory with its sidecar.
    let mut f = fixture("moovlast.mp4", Some(&script));
    let options = PrepareOptions {
        quarantine_dir: Some(f.base.join("q")),
        ..PrepareOptions::default()
    };
    let result = prepare_file(&mut f.ctx, &f.source, &options, None).unwrap();
    assert_eq!(
        result.error_kind.map(ErrorKind::as_str),
        Some("blockers"),
        "{:?}",
        result.error
    );
    assert_eq!(
        result.error.as_deref(),
        Some("compliance blockers in the output for moovlast.mp4")
    );
    let quarantined: Vec<String> = files_under(&f.base.join("q"))
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(quarantined, ["moovlast.mp4", "moovlast.mp4.verify.json"]);
    assert_eq!(leftovers(&f.base), Vec::<String>::new());

    // A caller that pinned the source itself never gets a quarantine move.
    let mut f = fixture("moovlast.mp4", Some(&script));
    let target = FdFixTarget::for_path(&f.source).unwrap();
    let result = prepare_file(
        &mut f.ctx,
        &f.source,
        &PrepareOptions::default(),
        Some(&target),
    )
    .unwrap();
    assert_eq!(result.error_kind.map(ErrorKind::as_str), Some("blockers"));
    assert!(!f.base.join("q").exists());
    assert_eq!(leftovers(&f.base), Vec::<String>::new());
}

fn write_srt(path: &Path, text: &str) {
    std::fs::write(path, format!("1\n00:00:00,000 --> 00:00:01,000\n{text}\n")).unwrap();
}

/// Prepares `name` next to `de.srt`, `de.sdh.srt` and `de.forced.srt`: afterwards every sidecar
/// counts as embedded, no sidecar is reported as still to merge, and a second prepare changes
/// nothing. The MP4 muxer drops subtitle titles, so SDH survives only as the `hearing_impaired`
/// disposition (#62).
fn assert_sdh_sidecars_stay_embedded(name: &str) {
    let mut f = fixture(name, None);
    let work = f.source.parent().unwrap().to_path_buf();
    let stem = f.source.file_stem().unwrap().to_string_lossy().into_owned();
    let sidecars: Vec<String> = ["de", "de.sdh", "de.forced"]
        .iter()
        .map(|kind| format!("{stem}.{kind}.srt"))
        .collect();
    for sidecar in &sidecars {
        write_srt(&work.join(sidecar), sidecar);
    }

    let first = prepare_file(&mut f.ctx, &f.source, &PrepareOptions::default(), None).unwrap();
    assert_eq!(first.error, None, "{name}: first prepare");
    let output = first.output.clone().unwrap_or_else(|| f.source.clone());
    let output_name = output.file_name().unwrap().to_string_lossy().into_owned();
    let probe = f.ctx.ffprobe.probe_json(&output).unwrap();
    let streams = probe["streams"].as_array().unwrap();
    let states: Vec<(String, String)> =
        atvprep_core::sidecars::classify_sidecars(&output_name, &sidecars, Some(streams))
            .into_iter()
            .map(|(n, _, s)| (n, s))
            .collect();
    let embedded: Vec<(String, String)> = sidecars
        .iter()
        .map(|n| (n.clone(), "embedded".to_owned()))
        .collect();
    assert_eq!(states, embedded, "{name}: sidecar states after prepare");

    let paths: Vec<PathBuf> = sidecars.iter().map(|n| work.join(n)).collect();
    let issues = atvprep_core::planner::detect_issues(
        &output,
        &probe,
        &paths,
        None,
        &f.ctx.ffprobe,
        &std::collections::BTreeSet::new(),
        atvprep_core::planner::DetectOptions {
            fix_dts: true,
            untagged_audio: false,
        },
    );
    assert!(
        !issues.iter().any(|i| i.contains("sidecar")),
        "{name}: {issues:?}"
    );

    let before = sha(&output);
    let signature_before = signature(&output, &f.ctx.ffprobe);
    let second = prepare_file(&mut f.ctx, &output, &PrepareOptions::default(), None).unwrap();
    assert_eq!(second.error, None, "{name}: second prepare");
    assert_eq!(
        sha(&output),
        before,
        "{name}: the second prepare rewrote the file"
    );
    assert_eq!(signature(&output, &f.ctx.ffprobe), signature_before);
}

#[test]
fn sdh_sidecars_embedded_into_an_mp4_count_as_embedded() {
    assert_sdh_sidecars_stay_embedded("clean.mp4");
}

#[test]
fn sdh_sidecars_embedded_from_an_mkv_count_as_embedded() {
    assert_sdh_sidecars_stay_embedded("movie.mkv");
}

/// A one-second MP4 whose German AC-3 5.1 track runs at 44.1 kHz, otherwise ready for the Apple TV.
fn ac3_at_44_1_khz(dir: &Path) -> PathBuf {
    let path = dir.join("Film.mp4");
    let status = std::process::Command::new("ffmpeg")
        .stdin(std::process::Stdio::null())
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100"])
        .args(["-t", "1", "-map", "0:v", "-map", "1:a"])
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-profile:v",
            "high",
        ])
        .args(["-c:a", "ac3", "-ac", "6", "-ar", "44100"])
        .args([
            "-metadata:s:v:0",
            "language=deu",
            "-metadata:s:a:0",
            "language=deu",
        ])
        .args(["-disposition:a:0", "default"])
        .args(["-movflags", "+faststart"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    path
}

fn rules(report: &atvprep_core::verify::VerifyReport) -> Vec<&str> {
    report.violations.iter().map(|v| v.rule.as_str()).collect()
}

#[test]
fn ac3_at_44_1_khz_is_resampled_to_48_khz_and_then_passes_the_check() {
    let mut f = fixture("clean.mp4", None);
    let source = ac3_at_44_1_khz(f.source.parent().unwrap());
    assert!(
        rules(&verify_file(&source, &SystemTools::new(), None, None))
            .contains(&"audio-sample-rate")
    );

    let target = FdFixTarget::for_path(&source).unwrap();
    let audio = PrepareOptions {
        encode: EncodeAllowance::Audio,
        ..PrepareOptions::default()
    };
    let planned = planned_change(&mut f.ctx, &source, &audio, &target).unwrap();
    assert_eq!(
        (planned.need.as_str(), planned.change.as_str()),
        ("audio", "audio")
    );
    // The planner names what it encodes, so nothing downstream has to guess codec or rate.
    assert_eq!(
        planned.steps,
        [PlanStep::SampleRate {
            stream: 1,
            from_codec: "ac3".to_owned(),
            to_codec: "eac3".to_owned(),
            from_hz: 44100,
            to_hz: 48000,
        }]
    );
    assert_eq!(
        planned.steps[0].text(),
        "audio stream #1 ac3 44100Hz -> eac3 48000Hz"
    );
    drop(target);

    // „Nur kopieren“ refuses and leaves the file as it was.
    let before = sha(&source);
    let copy = PrepareOptions {
        encode: EncodeAllowance::Copy,
        ..PrepareOptions::default()
    };
    let refused = prepare_file(&mut f.ctx, &source, &copy, None).unwrap();
    assert_eq!(refused.outcome.as_str(), "encode_not_allowed");
    assert_eq!(
        refused.reasons,
        [
            "no stereo track for deu",
            "audio stream #1 sample rate 44100Hz needs 48000Hz"
        ]
    );
    assert_eq!(sha(&source), before);

    let result = prepare_file(&mut f.ctx, &source, &audio, None).unwrap();
    assert_eq!(result.error, None);
    let output = result.output.unwrap_or_else(|| source.clone());
    let report = verify_file(&output, &SystemTools::new(), None, None);
    assert!(!rules(&report).contains(&"audio-sample-rate"), "{report:?}");

    let probe = f.ctx.ffprobe.probe_json(&output).unwrap();
    // Language and the Apple layout (the stereo track of the language is the enabled one) stay as
    // for any copy; the MP4 muxer keeps no track title apart from the handler name.
    let tracks: Vec<(&str, &str, &str, &str, i64)> = probe["streams"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .map(|s| {
            (
                s["codec_name"].as_str().unwrap(),
                s["sample_rate"].as_str().unwrap(),
                s["tags"]["language"].as_str().unwrap_or(""),
                s["tags"]["handler_name"].as_str().unwrap_or(""),
                s["disposition"]["default"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    assert_eq!(
        tracks,
        [
            ("eac3", "48000", "deu", "German", 0),
            ("aac", "48000", "deu", "German", 1),
        ]
    );
}

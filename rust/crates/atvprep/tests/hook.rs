//! The Servarr hook: argv, stdout, stderr and exit codes on top of the prepare pipeline.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use atvprep::{Environment, hook_main};
use atvprep_core::console::SharedBuffer;
use atvprep_core::hook::{destination_sidecars, exit_code};
use atvprep_core::prepare::ErrorKind;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn source(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/prepare")
        .join(name)
}

fn hook(args: &[&Path], ffmpeg: Option<&str>) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let mut env = Environment::from_process();
    env.ffmpeg = ffmpeg.map(|body| {
        let path = dir.path().join("ffmpeg");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    });
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let (mut out, err) = (Vec::new(), SharedBuffer::default());
    let code = hook_main::run(&args, &env, &mut out, Box::new(err.clone()));
    Run {
        code,
        stdout: String::from_utf8(out).unwrap(),
        stderr: err.contents(),
    }
}

fn staged(name: &str, as_name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(as_name);
    std::fs::copy(source(name), &path).unwrap();
    (dir, path)
}

#[test]
fn without_arguments_the_hook_exits_2() {
    let run = hook(&[], None);
    assert_eq!(run.code, 2);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr
            .contains("Usage: servarr_hook.py <sourcePath> <destinationFilePath>")
    );
    assert!(
        run.stderr
            .starts_with(&format!("atvprep {}\n", atvprep_core::VERSION))
    );
    assert_eq!(hook(&[Path::new("only-one")], None).code, 2);
}

#[test]
fn a_missing_source_exits_3_and_names_both_files() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing source file.mkv");
    let dest = dir
        .path()
        .join("library with spaces")
        .join("destination file.mkv");
    let run = hook(&[&missing, &dest], None);
    assert_eq!(run.code, 3);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr
            .contains("Start Conversion: missing source file.mkv -> destination file.mp4")
    );
    assert!(run.stderr.contains(
        "Fehler (analysis): cannot read missing source file.mkv: No such file or directory"
    ));
}

#[test]
fn a_file_that_is_not_media_is_an_analysis_failure() {
    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("junk.mkv");
    std::fs::write(&junk, "not media").unwrap();
    let run = hook(&[&junk, &dir.path().join("out.mkv")], None);
    assert_eq!(run.code, 3, "{}", run.stderr);
    assert!(run.stderr.contains("Fehler (analysis):"));
    assert_eq!(run.stdout, "");
}

#[test]
fn the_exit_code_follows_the_error_kind() {
    let table = [
        (Some(ErrorKind::Analysis), 3),
        (Some(ErrorKind::NoOutput), 4),
        (Some(ErrorKind::Ffmpeg), 5),
        (Some(ErrorKind::Blockers), 6),
        (Some(ErrorKind::Unsafe), 4),
        (Some(ErrorKind::Transcode), 4),
        (None, 4),
    ];
    for (kind, want) in table {
        assert_eq!(exit_code(kind), want, "{kind:?}");
    }
}

#[test]
fn ffmpeg_failures_map_to_their_exit_codes() {
    let (dir, src) = staged("moovlast.mp4", "Film.mp4");
    let dest = dir.path().join("lib").join("Film.mkv");
    let no_output = hook(&[&src, &dest], Some("exit 0"));
    assert_eq!(
        (no_output.code, no_output.stdout.as_str()),
        (4, ""),
        "{}",
        no_output.stderr
    );
    assert!(
        no_output
            .stderr
            .contains("Fehler (no_output): ffmpeg wrote no output for Film.mp4")
    );
    let failing = hook(&[&src, &dest], Some("exit 1"));
    assert_eq!(
        (failing.code, failing.stdout.as_str()),
        (5, ""),
        "{}",
        failing.stderr
    );
    assert!(failing.stderr.contains("Fehler (ffmpeg): "));
    let fragmented = source("../mp4/syn_fragmented.mp4");
    let script = format!(
        "for last; do :; done\ncp '{}' \"$last\"",
        fragmented.display()
    );
    let blocked = hook(&[&src, &dest], Some(&script));
    assert_eq!(
        (blocked.code, blocked.stdout.as_str()),
        (6, ""),
        "{}",
        blocked.stderr
    );
    assert!(
        blocked
            .stderr
            .contains("Fehler (blockers): compliance blockers in the output for Film.mp4")
    );
    assert!(
        blocked.stderr.contains("  [blocker] "),
        "{}",
        blocked.stderr
    );
}

#[test]
fn reencode_needed_is_not_success() {
    let (dir, src) = staged("high10.mp4", "Film.mp4");
    let before = std::fs::read(&src).unwrap();
    let run = hook(&[&src, &dir.path().join("Film.mp4")], None);
    assert_eq!((run.code, run.stdout.as_str()), (6, ""), "{}", run.stderr);
    assert!(run.stderr.contains(
        "Film.mp4: the video needs a re-encode; the file was left unchanged. Re-encode it from the manager."
    ));
    assert_eq!(std::fs::read(&src).unwrap(), before);
}

#[test]
fn a_video_encode_in_another_container_is_refused_too() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("Film.mkv");
    atvprep_testkit::remux(&source("high10.mp4"), &src);
    let before = std::fs::read(&src).unwrap();
    let dest = dir.path().join("lib").join("Film.mkv");
    let run = hook(&[&src, &dest], None);
    assert_eq!((run.code, run.stdout.as_str()), (6, ""), "{}", run.stderr);
    assert!(run.stderr.contains("Re-encode it from the manager."));
    assert_eq!(std::fs::read(&src).unwrap(), before);
    assert!(!dir.path().join("lib").exists(), "{}", run.stderr);
}

#[test]
fn an_audio_encode_runs_and_the_original_stays() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("Film.mp4");
    atvprep_testkit::write_surround_only_mp4(&src);
    let before = std::fs::read(&src).unwrap();
    let dest = dir.path().join("lib").join("Film.mp4");
    let run = hook(&[&src, &dest], None);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.starts_with("[MediaFile] "), "{}", run.stdout);
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a"])
        .args(["-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(dest.with_extension("mp4"))
        .output()
        .expect("ffprobe must be on PATH");
    assert!(String::from_utf8_lossy(&probe.stdout).contains("aac"));
    assert_eq!(
        std::fs::read(&src).unwrap(),
        before,
        "the hook never deletes"
    );
}

#[test]
fn success_prints_only_the_media_file_line_on_stdout() {
    let (dir, src) = staged("clean.mp4", "incoming file.mp4");
    let dest = dir.path().join("library with spaces").join("Film name.mkv");
    let run = hook(&[&src, &dest], None);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let expected = dest.parent().unwrap().join("Film name.mp4");
    let shown = expected.canonicalize().unwrap_or(expected);
    assert_eq!(run.stdout.lines().count(), 1, "{}", run.stdout);
    assert!(run.stdout.starts_with("[MediaFile] "));
    assert!(
        run.stdout.trim_end().ends_with("Film name.mp4"),
        "{}{shown:?}",
        run.stdout
    );
    assert!(
        run.stderr.contains("ATVPREP-STAGE finalize"),
        "{}",
        run.stderr
    );
    assert!(src.exists(), "the original is never deleted");
}

#[test]
fn a_destination_equal_to_a_clean_source_prints_the_source() {
    let (_dir, src) = staged("clean.mp4", "Film.mp4");
    let before = std::fs::read(&src).unwrap();
    let run = hook(&[&src, &src], None);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.trim_end().ends_with("Film.mp4"));
    assert_eq!(std::fs::read(&src).unwrap(), before);
}

#[test]
fn bazarr_sidecars_next_to_the_destination_are_merged() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("Film.mkv");
    for name in ["Film.en.srt", "Film.de.SRT", "Other.fr.srt", "Film.txt"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let found: Vec<String> = destination_sidecars(&dest)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(found, ["Film.de.SRT", "Film.en.srt"]);
    assert!(destination_sidecars(&dir.path().join("nowhere").join("Film.mkv")).is_empty());
}

#[test]
fn the_hook_runs_when_started_under_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("atvprep-hook");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_atvprep"), &link).unwrap();
    let output = std::process::Command::new(&link).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage: servarr_hook.py"));
    // The plain binary is the CLI, not the hook.
    let plain = std::process::Command::new(env!("CARGO_BIN_EXE_atvprep"))
        .output()
        .unwrap();
    assert_eq!(plain.status.code(), Some(1));
}

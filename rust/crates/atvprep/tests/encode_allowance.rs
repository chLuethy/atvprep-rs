//! `atvprep prepare --encode`: the Kodierfreigabe from the outside, with real tools (ADR 0008).

use std::ffi::OsString;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use atvprep::{Environment, Io, run};
use atvprep_core::console::SharedBuffer;
use atvprep_testkit::{remux, write_surround_only_mp4};

struct Ran {
    code: i32,
    out: String,
    err: String,
}

fn prepare(flags: &[&str], file: &Path) -> Ran {
    let (out, err) = (SharedBuffer::default(), SharedBuffer::default());
    let io = Io {
        stdin: Box::new(Cursor::new(Vec::new())),
        stdout: Box::new(out.clone()),
        stderr: Box::new(err.clone()),
        stdout_is_terminal: false,
        stdin_is_terminal: false,
    };
    let mut args: Vec<OsString> = vec!["prepare".into(), "--yes".into()];
    args.extend(flags.iter().map(OsString::from));
    args.push(file.into());
    let code = run(&args, &Environment::from_process(), io);
    Ran {
        code,
        out: out.contents(),
        err: err.contents(),
    }
}

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/prepare")
        .join(name)
}

/// `file.mp4` in a new directory: H.264 video that can be copied, EAC3 5.1 and no stereo track.
fn needs_audio() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.mp4");
    write_surround_only_mp4(&path);
    (dir, path)
}

/// `file.mp4` in a new directory: High 10 video, which has to be re-encoded.
fn needs_video() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.mp4");
    std::fs::copy(testdata("high10.mp4"), &path).unwrap();
    (dir, path)
}

fn in_mkv(mp4: &Path) -> PathBuf {
    let mkv = mp4.with_file_name("source.mkv");
    remux(mp4, &mkv);
    std::fs::remove_file(mp4).unwrap();
    mkv
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The codec names of the `kind` streams in `path`, in stream order.
fn codecs(path: &Path, kind: &str) -> Vec<String> {
    let output = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", kind])
        .args(["-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe must be on PATH");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// Refused as asked: exit 0, both machine lines, and nothing in the directory changed.
fn assert_not_allowed(ran: &Ran, file: &Path, before: &[u8], need: &str, allowance: &str) {
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    assert!(
        ran.out
            .lines()
            .any(|l| l == "ATVPREP-OUTCOME encode_not_allowed"),
        "{}",
        ran.out
    );
    let detail = format!("ATVPREP-ENCODE need={need} allowance={allowance}");
    assert!(ran.out.lines().any(|l| l == detail), "{}", ran.out);
    assert_eq!(std::fs::read(file).unwrap(), before, "{}", ran.out);
    assert_eq!(
        names_in(file.parent().unwrap()),
        [file.file_name().unwrap().to_string_lossy()],
        "{}",
        ran.out
    );
}

#[test]
fn copy_refuses_an_audio_encode_and_leaves_the_file_alone() {
    let (_dir, file) = needs_audio();
    let before = std::fs::read(&file).unwrap();
    let ran = prepare(&["--encode", "copy"], &file);
    assert_not_allowed(&ran, &file, &before, "audio", "copy");
}

#[test]
fn copy_refuses_a_video_encode() {
    let (_dir, file) = needs_video();
    let before = std::fs::read(&file).unwrap();
    let ran = prepare(&["--encode", "copy"], &file);
    assert_not_allowed(&ran, &file, &before, "video", "copy");
}

#[test]
fn copy_still_rebuilds_what_only_needs_a_remux() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file.mp4");
    std::fs::copy(testdata("moovlast.mp4"), &file).unwrap();
    let ran = prepare(&["--encode", "copy"], &file);
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    assert!(ran.out.contains("ATVPREP-OUTCOME rebuilt"), "{}", ran.out);
}

#[test]
fn audio_adds_the_stereo_track_and_keeps_the_video() {
    let (_dir, file) = needs_audio();
    let ran = prepare(&["--encode", "audio"], &file);
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    assert!(ran.out.contains("ATVPREP-OUTCOME rebuilt"), "{}", ran.out);
    assert!(
        codecs(&file, "a").contains(&"aac".to_owned()),
        "{}",
        ran.out
    );
    assert_eq!(codecs(&file, "v"), ["h264"]);
}

#[test]
fn audio_refuses_a_video_encode() {
    let (_dir, file) = needs_video();
    let before = std::fs::read(&file).unwrap();
    let ran = prepare(&["--encode", "audio"], &file);
    assert_not_allowed(&ran, &file, &before, "video", "audio");
    assert!(ran.out.contains("--encode all"), "{}", ran.out);
}

#[test]
fn all_re_encodes_an_mp4_without_delete_original() {
    let (dir, file) = needs_video();
    let ran = prepare(&["--encode", "all"], &file);
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    assert!(ran.out.contains("ATVPREP-OUTCOME reencoded"), "{}", ran.out);
    assert_eq!(codecs(&file, "v"), ["hevc"]);
    assert_eq!(names_in(dir.path()), ["file.mp4"]);
}

#[test]
fn without_the_flag_audio_applies_to_an_mp4() {
    let (_dir, file) = needs_audio();
    let ran = prepare(&[], &file);
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    assert!(
        codecs(&file, "a").contains(&"aac".to_owned()),
        "{}",
        ran.out
    );

    let (_dir, file) = needs_video();
    let before = std::fs::read(&file).unwrap();
    let ran = prepare(&[], &file);
    assert_not_allowed(&ran, &file, &before, "video", "audio");
}

#[test]
fn without_the_flag_audio_applies_to_another_container() {
    let (dir, mp4) = needs_audio();
    let mkv = in_mkv(&mp4);
    let ran = prepare(&[], &mkv);
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    let made = dir.path().join("source.mp4");
    assert!(
        codecs(&made, "a").contains(&"aac".to_owned()),
        "{}",
        ran.out
    );
    assert!(mkv.exists(), "the original stays without --delete-original");

    let (_dir, mp4) = needs_video();
    let mkv = in_mkv(&mp4);
    let before = std::fs::read(&mkv).unwrap();
    let ran = prepare(&[], &mkv);
    assert_not_allowed(&ran, &mkv, &before, "video", "audio");
}

#[test]
fn a_refusal_comes_before_the_first_write_to_an_output_path() {
    let (dir, file) = needs_video();
    let before = std::fs::read(&file).unwrap();
    let out = dir.path().join("new").join("sub").join("out.mp4");
    let ran = prepare(&["-o", out.to_str().unwrap()], &file);
    assert_not_allowed(&ran, &file, &before, "video", "audio");
    assert!(!dir.path().join("new").exists(), "{}", ran.out);
}

#[test]
fn a_dry_run_reports_the_same_refusal() {
    let (dir, mp4) = needs_video();
    let mkv = in_mkv(&mp4);
    let before = std::fs::read(&mkv).unwrap();
    let ran = prepare(&["--dry-run"], &mkv);
    assert_not_allowed(&ran, &mkv, &before, "video", "audio");
    assert_eq!(names_in(dir.path()), ["source.mkv"]);
}

#[test]
fn a_refused_sibling_mp4_leaves_both_files_alone() {
    let (dir, mp4) = needs_video();
    let mkv = mp4.with_file_name("file.mkv");
    remux(&mp4, &mkv);
    let (mp4_before, mkv_before) = (std::fs::read(&mp4).unwrap(), std::fs::read(&mkv).unwrap());
    let ran = prepare(&["--delete-original"], &mkv);
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    assert!(
        ran.out
            .contains("ATVPREP-ENCODE need=video allowance=audio"),
        "{}",
        ran.out
    );
    assert!(!ran.out.contains("ATVPREP-SOURCE"), "{}", ran.out);
    assert_eq!(std::fs::read(&mp4).unwrap(), mp4_before);
    assert_eq!(std::fs::read(&mkv).unwrap(), mkv_before);
    assert_eq!(names_in(dir.path()), ["file.mkv", "file.mp4"]);
}

#[test]
fn copy_only_is_gone() {
    let (_dir, file) = needs_audio();
    let before = std::fs::read(&file).unwrap();
    let ran = prepare(&["--copy-only"], &file);
    assert_eq!(ran.code, 2, "{}{}", ran.out, ran.err);
    assert_eq!(std::fs::read(&file).unwrap(), before);
}

#[test]
fn an_unknown_level_is_a_usage_error() {
    let (_dir, file) = needs_audio();
    assert_eq!(prepare(&["--encode", "video"], &file).code, 2);
}

#[test]
fn delete_original_does_not_allow_an_encode() {
    let (dir, file) = needs_video();
    let before = std::fs::read(&file).unwrap();
    let ran = prepare(&["--delete-original"], &file);
    assert_not_allowed(&ran, &file, &before, "video", "audio");
    assert_eq!(names_in(dir.path()), ["file.mp4"]);
}

#[test]
fn delete_original_removes_another_container_after_a_verified_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mkv = dir.path().join("file.mkv");
    std::fs::copy(testdata("movie.mkv"), &mkv).unwrap();
    let ran = prepare(&["--encode", "copy", "--delete-original"], &mkv);
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    assert!(ran.out.contains("ATVPREP-SOURCE removed"), "{}", ran.out);
    assert_eq!(names_in(dir.path()), ["file.mp4"]);
}

#[test]
fn delete_original_never_removes_an_mp4() {
    let (dir, file) = needs_video();
    let ran = prepare(&["--encode", "all", "--delete-original"], &file);
    assert_eq!(ran.code, 0, "{}{}", ran.out, ran.err);
    assert!(!ran.out.contains("ATVPREP-SOURCE"), "{}", ran.out);
    assert_eq!(names_in(dir.path()), ["file.mp4"]);
}

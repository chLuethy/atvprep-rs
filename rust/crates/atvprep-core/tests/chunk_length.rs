//! The chunk length of the target file (#144, ADR 0010): `planned_change` and the compliance rule detect
//! files with chunks that are too small from the moov and classify them as a remux, never a patch.
//!
//! Needs ffmpeg, ffprobe and Bento4 on `PATH`; the tests that need a good file also need
//! `MP4Box` and are skipped without it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use atvprep_core::compliance::SystemTools;
use atvprep_core::console::{Console, SharedBuffer};
use atvprep_core::encode::ChangeKind;
use atvprep_core::fd_fix_contract::FdFixTarget;
use atvprep_core::ffprobe::Ffprobe;
use atvprep_core::mp4::chunks::CHUNK_RULE;
use atvprep_core::pipeline_postfix::RunContext;
use atvprep_core::prepare::PrepareOptions;
use atvprep_core::prepare::chunks::Mp4Box;
use atvprep_core::prepare::pipeline::planned_change;
use atvprep_core::stages::StageReporter;
use atvprep_core::verify::verify_file;

fn context() -> RunContext {
    RunContext {
        console: Console::recording(false).0,
        reporter: StageReporter::new(Box::new(SharedBuffer::default()), false),
        tools: Box::new(SystemTools::new()),
        ffprobe: Ffprobe::new(),
        ffmpeg: None,
        mp4box: Mp4Box::new(),
    }
}

fn has_mp4box() -> bool {
    Command::new("MP4Box")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Twenty seconds of video and stereo audio as ffmpeg writes them with default settings:
/// chunks of one or two samples. Otherwise ready for Apple TV.
fn ffmpeg_file(dir: &Path) -> PathBuf {
    let path = dir.join("Film.mp4");
    let status = Command::new("ffmpeg")
        .stdin(Stdio::null())
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-t", "20", "-map", "0:v", "-map", "1:a"])
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-profile:v",
            "high",
        ])
        .args(["-c:a", "aac", "-ac", "2"])
        .args([
            "-metadata:s:v:0",
            "language=deu",
            "-metadata:s:a:0",
            "language=deu",
        ])
        .args(["-disposition:a:0", "default", "-movflags", "+faststart"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    path
}

/// `source` with chunks of about 0.5 s, rewritten by `MP4Box`.
fn with_half_second_chunks(source: &Path, dir: &Path) -> PathBuf {
    let path = dir.join("Rewritten.mp4");
    let status = Command::new("MP4Box")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args(["-inter", "500"])
        .arg(source)
        .arg("-out")
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    path
}

fn change_of(path: &Path) -> Option<ChangeKind> {
    let target = FdFixTarget::for_path(path).unwrap();
    planned_change(&mut context(), path, &PrepareOptions::default(), &target).map(|p| p.change)
}

fn rules(path: &Path) -> Vec<String> {
    verify_file(path, &SystemTools::new(), None, None)
        .violations
        .into_iter()
        .map(|v| v.rule)
        .collect()
}

#[test]
fn a_file_with_chunks_of_one_or_two_samples_is_a_remux_and_shows_the_chunk_rule() {
    let dir = tempfile::tempdir().unwrap();
    let file = ffmpeg_file(dir.path());
    assert_eq!(change_of(&file), Some(ChangeKind::Remux));
    assert!(rules(&file).contains(&CHUNK_RULE.to_owned()));
}

#[test]
fn a_file_with_chunks_of_half_a_second_has_no_chunk_finding() {
    if !has_mp4box() {
        eprintln!("MP4Box missing: skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let file = with_half_second_chunks(&ffmpeg_file(dir.path()), out.path());
    assert_ne!(change_of(&file), Some(ChangeKind::Remux));
    assert!(!rules(&file).contains(&CHUNK_RULE.to_owned()));
}

//! The Sonarr/Radarr custom-script hook (port of `atvprep/servarr_hook.py`).
//!
//! `atvprep-hook <sourcePath> <destinationFilePath>`: stdout carries only `[MediaFile] <path>`;
//! everything else goes to the console, which the caller points at stderr. Exit codes: 0 ok,
//! 2 usage, 3 analysis (including a missing or unreadable source), 4 no output or any other
//! failure, 5 ffmpeg, 6 compliance blockers or a video that needs a re-encode.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::pipeline_postfix::RunContext;
use crate::prepare::paths::resolve;
use crate::prepare::pipeline::{PrepareError, prepare_file};
use crate::prepare::{ErrorKind, Outcome, PrepareOptions};
use crate::sidecars::find_sidecar_srts;

/// The exit code for a failed run of the given kind.
#[must_use]
pub const fn exit_code(kind: Option<ErrorKind>) -> i32 {
    match kind {
        Some(ErrorKind::Analysis) => 3,
        Some(ErrorKind::Ffmpeg) => 5,
        Some(ErrorKind::Blockers) => 6,
        _ => 4,
    }
}

/// The SRTs Bazarr placed next to the destination (library) file; the pipeline finds those next to
/// the source on its own.
#[must_use]
pub fn destination_sidecars(dest: &Path) -> Vec<PathBuf> {
    let there = dest.parent().is_some_and(Path::is_dir);
    if there {
        find_sidecar_srts(dest).unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// `strerror` of an I/O error: the message without Rust's ` (os error N)` suffix.
fn strerror(error: &std::io::Error) -> String {
    let text = error.to_string();
    text.split(" (os error").next().unwrap_or(&text).to_owned()
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Run the hook. `args` are the arguments after the program name.
pub fn run_hook(args: &[OsString], ctx: &mut RunContext, stdout: &mut dyn Write) -> i32 {
    ctx.console.print(&format!("atvprep {}", crate::VERSION));
    let [source, dest, ..] = args else {
        ctx.console
            .print("Usage: servarr_hook.py <sourcePath> <destinationFilePath>");
        return 2;
    };
    let source = resolve(Path::new(source));
    let dest = resolve(Path::new(dest));
    let stem = dest
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let final_mp4 = dest.parent().map_or_else(
        || PathBuf::from(format!("{stem}.mp4")),
        |p| p.join(format!("{stem}.mp4")),
    );
    ctx.console.print(&format!(
        "Start Conversion: {} -> {}",
        name_of(&source),
        name_of(&final_mp4)
    ));

    // A source that cannot be read is an analysis failure (3), not a generic one.
    if let Err(error) = std::fs::File::open(&source) {
        ctx.console.print(&format!(
            "Fehler (analysis): cannot read {}: {}",
            name_of(&source),
            strerror(&error)
        ));
        return 3;
    }

    // The original is never deleted: Sonarr/Radarr own it.
    let options = PrepareOptions {
        output: Some(final_mp4.clone()),
        extra_sidecars: destination_sidecars(&dest),
        ..PrepareOptions::default()
    };
    let result = match prepare_file(ctx, &source, &options, None) {
        Ok(result) => result,
        Err(error) => {
            let kind = match error {
                PrepareError::Usage(_) => "ValueError",
                PrepareError::Other(_) => "Error",
            };
            ctx.console
                .print(&format!("Fehler (unexpected): {kind}: {error}"));
            return 4;
        }
    };

    if result.outcome == Outcome::ReencodeNeeded {
        // Re-encoding an MP4 in place needs the user's decision; the hook neither does it nor
        // reports success.
        ctx.console.print(&format!(
            "{}: the video needs a re-encode; the file was left unchanged. Re-encode it from the manager.",
            name_of(&source)
        ));
        return 6;
    }
    if !result.ok() {
        let kind = result.error_kind.map_or("", ErrorKind::as_str);
        ctx.console.print(&format!(
            "Fehler ({kind}): {}",
            result.error.as_deref().unwrap_or_default()
        ));
        for v in result.report.iter().flat_map(|r| r.blockers()) {
            let track = v
                .track_id
                .map_or_else(String::new, |t| format!(" (track {t})"));
            ctx.console
                .print(&format!("  [blocker] {}{track}: {}", v.rule, v.message));
        }
        return exit_code(result.error_kind);
    }

    // No output means the destination is the source MP4 itself and it was already clean.
    let media = result.output.unwrap_or(final_mp4);
    // Sonarr and Radarr read this line, so a failed write must not look like success.
    if writeln!(stdout, "[MediaFile] {}", media.display()).is_err() || stdout.flush().is_err() {
        return exit_code(None);
    }
    0
}

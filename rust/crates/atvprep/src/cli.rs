//! The commands: what each one reads, does and prints (port of `atvprep/__main__.py`).

use std::io::BufRead;
use std::path::{Path, PathBuf};

use atvprep_core::constants::VIDEO_EXTS;
use atvprep_core::fd_fix_contract::{FdFixTarget, UnsafeFixTarget};
use atvprep_core::pipeline_postfix::RunContext;
use atvprep_core::prepare::paths::resolve;
use atvprep_core::prepare::pipeline::{patch_media, prepare_file};
use atvprep_core::prepare::{PrepareOptions, PrepareResult};
use atvprep_core::probe_helpers::subtitle_infos;
use atvprep_core::pyjson::dumps_indent2;
use atvprep_core::stages::Outcome;
use atvprep_core::verify::{VerifyReport, verify_tree};
use clap::ArgMatches;
use serde::Serialize;

use crate::Environment;
use crate::sink::SharedSink;

const VERIFY_EXTS: [&str; 3] = [".mp4", ".mov", ".m4v"];

fn flag(matches: &ArgMatches, name: &str) -> bool {
    matches.get_flag(name)
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn suffix_lower(path: &Path) -> String {
    path.extension().map_or_else(String::new, |e| {
        format!(".{}", e.to_string_lossy().to_lowercase())
    })
}

/// Every file below `dir`, sorted by path; symlinked directories are not entered.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&path, out),
            _ if path.is_file() => out.push(path),
            _ => {}
        }
    }
}

fn collect_files(
    ctx: &mut RunContext,
    inputs: &[String],
    exts: &[&str],
    label: &str,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for input in inputs {
        let path = resolve(Path::new(input));
        if path.is_dir() {
            let mut found = Vec::new();
            walk(&path, &mut found);
            found.sort();
            files.extend(
                found
                    .into_iter()
                    .filter(|f| exts.contains(&suffix_lower(f).as_str())),
            );
        } else if path.is_file() {
            if exts.contains(&suffix_lower(&path).as_str()) {
                files.push(path);
            } else {
                ctx.console.print(&format!(
                    "Warning: {} is not a {label}; skipped",
                    name_of(&path)
                ));
            }
        } else {
            ctx.console
                .print(&format!("Warning: input path not found: {input}"));
        }
    }
    files
}

fn strings(matches: &ArgMatches, name: &str) -> Vec<String> {
    matches
        .get_many::<String>(name)
        .map(|v| v.cloned().collect())
        .unwrap_or_default()
}

/// `atvprep patch`: `alternate_group` and the Apple elng/chtx atoms in existing MP4s.
pub fn run_patch(ctx: &mut RunContext, matches: &ArgMatches) -> i32 {
    let verbose = flag(matches, "verbose");
    let files = collect_files(ctx, &strings(matches, "inputs"), &[".mp4"], ".mp4");
    if files.is_empty() {
        ctx.console.print("No MP4 files found.");
        return 1;
    }
    for file in files {
        ctx.console.print(&format!("Patching: {}", name_of(&file)));
        ctx.reporter.begin_file(false);
        let outcome = ctx
            .ffprobe
            .probe_json(&file)
            .map_err(|e| e.to_string())
            .and_then(|probe| {
                let infos = subtitle_infos(&probe);
                patch_media(ctx, &file, &infos, None, verbose, false).map_err(|e| e.to_string())
            });
        if let Err(error) = outcome {
            ctx.console.print(&format!("  Error: {error}"));
        }
    }
    0
}

fn usage_error(ctx: &mut RunContext, message: &str) -> i32 {
    ctx.console.print(&format!("Error: {message}"));
    2
}

fn int(matches: &ArgMatches, name: &str) -> Option<i128> {
    matches.get_one::<i128>(name).copied()
}

/// The private manager contract, rejecting partial or unsafe values.
fn manager_target(matches: &ArgMatches) -> Result<Option<FdFixTarget>, UnsafeFixTarget> {
    let fd = int(matches, "manager-parent-fd");
    let leaf = matches.get_one::<String>("manager-leaf");
    let (device, inode, ctime) = (
        int(matches, "manager-device"),
        int(matches, "manager-inode"),
        int(matches, "manager-ctime-ns"),
    );
    if fd.is_none() && leaf.is_none() && device.is_none() && inode.is_none() && ctime.is_none() {
        return Ok(None);
    }
    let (Some(fd), Some(leaf), Some(device), Some(inode), Some(ctime)) =
        (fd, leaf, device, inode, ctime)
    else {
        return Err(UnsafeFixTarget(
            "incomplete manager fix contract".to_owned(),
        ));
    };
    let fd = i32::try_from(fd)
        .map_err(|_| UnsafeFixTarget("incomplete manager fix contract".to_owned()))?;
    FdFixTarget::from_contract(fd, leaf, device, inode, ctime).map(Some)
}

/// `--quarantine DIR` wins over `ATVPREP_QUARANTINE`; unset or empty means no quarantine.
pub(crate) fn quarantine_dir(matches: &ArgMatches, env: &Environment) -> Option<PathBuf> {
    let value = matches
        .try_get_one::<String>("quarantine")
        .ok()
        .flatten()
        .filter(|v| !v.is_empty())
        .cloned()
        .or_else(|| env.quarantine.clone().filter(|v| !v.is_empty()))?;
    Some(resolve(&expand_user(&value)))
}

fn expand_user(value: &str) -> PathBuf {
    match (value.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        (None, Some(home)) if value == "~" => PathBuf::from(home),
        _ => PathBuf::from(value),
    }
}

fn answer(ctx: &mut RunContext, stdin: &mut dyn BufRead, prompt: &str) -> String {
    ctx.console.prompt(prompt);
    let mut line = String::new();
    let _ = stdin.read_line(&mut line);
    line.trim().to_lowercase()
}

/// One question for the whole batch; `-y` and a non-terminal stdin proceed without asking.
fn confirm_batch(
    ctx: &mut RunContext,
    matches: &ArgMatches,
    stdin: &mut dyn BufRead,
    stdin_is_terminal: bool,
    jobs: &[(PathBuf, PathBuf)],
) -> bool {
    if flag(matches, "yes") {
        return true;
    }
    if !stdin_is_terminal {
        ctx.console
            .print("Non-interactive: proceeding automatically.");
        return true;
    }
    let reply = if jobs.len() == 1 {
        answer(ctx, stdin, "Proceed? [Y/n] ")
    } else {
        ctx.console
            .print(&format!("\n{} file(s) queued:", jobs.len()));
        for (i, (src, dst)) in jobs.iter().enumerate() {
            ctx.console
                .print(&format!("  {}. {} → {}", i + 1, name_of(src), name_of(dst)));
        }
        ctx.console.print("");
        answer(
            ctx,
            stdin,
            &format!("Process all {} file(s)? [Y/n] ", jobs.len()),
        )
    };
    if !reply.is_empty() && reply != "y" && reply != "yes" {
        ctx.console.print("Skipped.");
        return false;
    }
    true
}

/// Flag combinations that cannot go together end the run with exit code 2.
fn reject_flag_mix(ctx: &mut RunContext, matches: &ArgMatches, quarantine: bool) -> Option<i32> {
    let manager = [
        "manager-parent-fd",
        "manager-leaf",
        "manager-device",
        "manager-inode",
        "manager-ctime-ns",
    ]
    .iter()
    .any(|name| matches.contains_id(name));
    let output = matches.contains_id("output");
    if manager && output {
        return Some(usage_error(
            ctx,
            "-o cannot be combined with the manager contract",
        ));
    }
    if manager && quarantine {
        return Some(usage_error(
            ctx,
            "the manager contract does not permit quarantine moves",
        ));
    }
    if flag(matches, "delete-original") && output {
        return Some(usage_error(
            ctx,
            "--delete-original cannot be combined with -o (the input is never touched then)",
        ));
    }
    None
}

fn prepare_options(
    matches: &ArgMatches,
    output: Option<PathBuf>,
    quarantine_dir: Option<PathBuf>,
) -> PrepareOptions {
    PrepareOptions {
        output,
        force: flag(matches, "force"),
        dry_run: flag(matches, "dry-run"),
        delete_original: flag(matches, "delete-original"),
        prefer_hevc10: flag(matches, "prefer-hevc10"),
        crf: matches.get_one::<u32>("crf").copied().unwrap_or(18),
        keep_av1: flag(matches, "keep-av1"),
        fix_dts: flag(matches, "fix-dts"),
        quarantine_dir,
        verbose: flag(matches, "verbose"),
        extra_sidecars: Vec::new(),
        copy_only: flag(matches, "copy-only"),
    }
}

/// `atvprep prepare` (also `fix` and `convert`). Exit 0 when every file is ok, 1 when one failed
/// or no input was found, 2 for flags that cannot go together.
pub fn run_prepare(
    ctx: &mut RunContext,
    env: &Environment,
    stdin: &mut dyn BufRead,
    stdin_is_terminal: bool,
    matches: &ArgMatches,
) -> i32 {
    let quarantine = quarantine_dir(matches, env);
    if let Some(code) = reject_flag_mix(ctx, matches, quarantine.is_some()) {
        return code;
    }
    // The manager cancels a run by sending SIGTERM to the whole process group. ffmpeg ends on it;
    // this process must outlive it for the one step that removes the partial file, so it only
    // notes the signal instead of dying on the spot. The manager's KILL after its grace period
    // still ends a run that does not follow.
    let terminated = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _ = signal_hook::flag::register(
        signal_hook::consts::SIGTERM,
        std::sync::Arc::clone(&terminated),
    );
    let target = match manager_target(matches) {
        Ok(target) => target,
        Err(error) => {
            ctx.console.print(&format!("Error: {error}"));
            return 1;
        }
    };
    let files = match &target {
        Some(target) => vec![target.path()],
        None => collect_files(ctx, &strings(matches, "inputs"), VIDEO_EXTS, "video file"),
    };
    if files.is_empty() {
        ctx.console.print("No video files found.");
        return 1;
    }
    // -o: a path ending in .mp4 is the output file, anything else a directory (created when missing).
    let out = matches
        .get_one::<String>("output")
        .map(|o| resolve(Path::new(o)));
    let (output_file, output_dir) = match out {
        Some(p) if suffix_lower(&p) == ".mp4" => (Some(p), None),
        Some(p) => (None, Some(p)),
        None => (None, None),
    };
    if let Some(file) = &output_file
        && files.len() > 1
    {
        return usage_error(
            ctx,
            &format!(
                "-o '{}' is a single .mp4 path but {} input files were found. Specify an output \
directory (without .mp4 suffix).",
                name_of(file),
                files.len()
            ),
        );
    }
    let output_for = |f: &Path| -> Option<PathBuf> {
        output_dir.as_ref().map_or_else(
            || output_file.clone(),
            |dir| Some(dir.join(format!("{}.mp4", stem_of(f)))),
        )
    };
    let dry_run = flag(matches, "dry-run");
    if !dry_run {
        let jobs: Vec<(PathBuf, PathBuf)> = files
            .iter()
            .map(|f| {
                let dst = output_for(f)
                    .unwrap_or_else(|| f.with_file_name(format!("{}.mp4", stem_of(f))));
                (f.clone(), dst)
            })
            .collect();
        if !confirm_batch(ctx, matches, stdin, stdin_is_terminal, &jobs) {
            return 0;
        }
        if let Some(dir) = &output_dir {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    let (mut clean, mut repaired, mut failed) = (0_u32, 0_u32, 0_u32);
    for file in &files {
        let options = prepare_options(matches, output_for(file), quarantine.clone());
        let result = match prepare_file(ctx, file, &options, target.as_ref()) {
            Ok(result) => result,
            Err(error) => {
                ctx.console
                    .print(&format!("Error processing {}: {error}", name_of(file)));
                ctx.reporter.outcome(Outcome::Failed);
                failed += 1;
                continue;
            }
        };
        report(ctx, &result);
        if !result.ok() {
            failed += 1;
        } else if result.outcome == Outcome::Done {
            clean += 1;
        } else if result.outcome == Outcome::Patched {
            repaired += 1;
        }
    }
    if clean > 0 && !dry_run {
        ctx.console
            .print(&format!("{clean} file(s) skipped (already optimal)"));
    }
    if repaired > 0 && !dry_run {
        ctx.console
            .print(&format!("{repaired} file(s) repaired in place"));
    }
    i32::from(failed > 0)
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn bullets(ctx: &mut RunContext, reasons: &[String]) {
    for reason in reasons {
        ctx.console.print(&format!("  • {reason}"));
    }
}

/// The human lines for one file, then the machine lines for a caller reading a pipe.
pub(crate) fn report(ctx: &mut RunContext, result: &PrepareResult) {
    use atvprep_core::escalation::Level;
    let name = name_of(&result.source);
    if result.outcome == Outcome::ReencodeNeeded {
        ctx.console.print(&format!(
            "re-encode needed: {name} (left untouched; run with --delete-original to replace it)"
        ));
        bullets(ctx, &result.reasons);
    } else if result.level == Level::None {
        if result.ok() {
            ctx.console
                .print(&format!("skip: {name} (already optimal)"));
        }
    } else {
        let label = match result.level {
            Level::InPlace if result.tags_only => "patch-only:",
            Level::InPlace => "postfix-only:",
            Level::Rebuild => "rebuild:",
            _ => "re-encode:",
        };
        ctx.console.print(&format!("{label} {name}"));
        bullets(ctx, &result.reasons);
    }
    if result.ok() {
        if let Some(output) = result.output.as_ref().filter(|o| **o != result.source) {
            ctx.console.print(&format!("  → {}", output.display()));
        }
    } else {
        let error = result.error.as_deref().unwrap_or_default();
        ctx.console
            .print(&format!("Error processing {name}: {error}"));
        if let Some(report) = &result.report {
            for v in report.blockers() {
                ctx.console.print(&format!("  ✗ {}: {}", v.rule, v.message));
            }
        }
    }
    if result.source_removed {
        ctx.console.print("  original removed");
    } else if let Some(kept) = &result.source_kept {
        ctx.console.print(&format!("  ! {kept}"));
    }
    ctx.reporter.outcome(result.outcome);
    if result.source_removed || result.source_kept.is_some() {
        ctx.reporter
            .source(result.source_removed, result.source_kept.as_deref());
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    path: String,
    passed: bool,
    violations: &'a [atvprep_core::compliance::Violation],
}

#[derive(Serialize)]
struct JsonDocument<'a> {
    files_checked: usize,
    files_passed: usize,
    files_failed: usize,
    total_blockers: usize,
    total_warnings: usize,
    reports: Vec<JsonReport<'a>>,
}

/// The `--json` text for `reports`, as Python's `json.dumps(..., indent=2)` prints it.
#[must_use]
pub fn reports_to_json(reports: &[VerifyReport]) -> String {
    let passed = reports.iter().filter(|r| r.passed()).count();
    let document = JsonDocument {
        files_checked: reports.len(),
        files_passed: passed,
        files_failed: reports.len() - passed,
        total_blockers: reports.iter().map(|r| r.blockers().len()).sum(),
        total_warnings: reports.iter().map(|r| r.warnings().len()).sum(),
        reports: reports
            .iter()
            .map(|r| JsonReport {
                path: r.path.to_string_lossy().into_owned(),
                passed: r.passed(),
                violations: &r.violations,
            })
            .collect(),
    };
    dumps_indent2(&document)
}

fn print_verify_human(ctx: &mut RunContext, reports: &[VerifyReport], verbose: bool) {
    let (mut blockers, mut warnings, mut passed, mut failed) = (0, 0, 0, 0);
    for report in reports {
        blockers += report.blockers().len();
        warnings += report.warnings().len();
        if report.passed() {
            passed += 1;
            if !verbose && report.warnings().is_empty() {
                continue;
            }
            ctx.console.print(&format!("✓ {}", report.path.display()));
        } else {
            failed += 1;
            ctx.console.print(&format!("✗ {}", report.path.display()));
        }
        for v in &report.violations {
            let track = v
                .track_id
                .map_or_else(String::new, |t| format!(" (track {t})"));
            ctx.console.print(&format!(
                "  [{}] {}{track}: {}",
                v.severity.as_str(),
                v.rule,
                v.message
            ));
        }
    }
    ctx.console.print(&format!(
        "\n{} file(s) checked — {passed} pass, {failed} fail ({blockers} blocker(s), {warnings} warning(s) total)",
        reports.len()
    ));
}

/// `atvprep verify`: exit 0 on a clean pass, 1 on a blocker (or a warning with
/// `--fail-on warning`), 2 for a missing or unknown path.
pub fn run_verify(ctx: &mut RunContext, sink: &SharedSink, matches: &ArgMatches) -> i32 {
    use std::io::Write as _;
    let pinned = match manager_target(matches) {
        Ok(target) => target,
        Err(error) => {
            ctx.console.print(&format!("Error: {error}"));
            return 1;
        }
    };
    let given = matches
        .get_one::<String>("path")
        .or_else(|| matches.get_one::<String>("scan"));
    let Some(given) = given else {
        ctx.console
            .print("Error: verify requires a path (file or directory)");
        return 2;
    };
    let opened = match pinned.as_ref().map(FdFixTarget::open_for_tools).transpose() {
        Ok(value) => value,
        Err(error) => {
            ctx.console.print(&format!("Error: {error}"));
            return 1;
        }
    };
    let target = if let Some(file) = &opened {
        use std::os::fd::AsRawFd;
        PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()))
    } else {
        resolve(Path::new(given))
    };
    if !target.exists() {
        ctx.console
            .print(&format!("Error: path not found: {}", target.display()));
        return 2;
    }
    let reports = if pinned.is_some() {
        vec![atvprep_core::verify::verify_file(
            &target,
            ctx.tools.as_ref(),
            None,
            None,
        )]
    } else {
        verify_tree(&target, ctx.tools.as_ref(), Some(&VERIFY_EXTS))
    };
    let json = flag(matches, "json");
    if reports.is_empty() {
        if json {
            let _ = writeln!(sink.clone(), "{}", reports_to_json(&reports));
        } else {
            ctx.console
                .print(&format!("No media files found under: {}", target.display()));
        }
        return 0;
    }
    if json {
        let _ = writeln!(sink.clone(), "{}", reports_to_json(&reports));
    } else {
        print_verify_human(ctx, &reports, flag(matches, "verbose"));
    }
    let any_blocker = reports.iter().any(|r| !r.blockers().is_empty());
    let any_warning = reports.iter().any(|r| !r.warnings().is_empty());
    let warning_fails = matches.get_one::<String>("fail-on").map(String::as_str) == Some("warning");
    i32::from(any_blocker || (warning_fails && any_warning))
}

/// `atvprep watch` until Ctrl+C.
pub fn run_watch(ctx: &mut RunContext, env: &Environment, matches: &ArgMatches) -> i32 {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    let stop = Arc::new(AtomicBool::new(false));
    // A failed registration only costs the graceful stop message; the default SIGINT still ends it.
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&stop));
    let waiting = Arc::clone(&stop);
    let sleep = move |total: Duration| {
        let slice = Duration::from_millis(100);
        let mut left = total;
        while !left.is_zero() && !waiting.load(Ordering::Relaxed) {
            let chunk = left.min(slice);
            std::thread::sleep(chunk);
            left -= chunk;
        }
    };
    crate::watch::run(ctx, env, matches, &stop, &sleep)
}

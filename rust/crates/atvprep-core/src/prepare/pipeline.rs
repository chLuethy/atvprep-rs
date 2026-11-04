//! The prepare pipeline: pick the cheapest safe route for one file and run it.
//!
//! Routes (see [`prepare_file`]): none / in-place patches on the source through its pinned
//! target, a patched copy or an ffmpeg build at an explicit output, an ffmpeg build replacing an
//! MP4 source, and an ffmpeg build of a new `<stem>.mp4` next to another container.

use std::fmt;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::checks::{subtitle_sidecars, verify_output_streams, verify_subtitle_inputs};
use super::ffmpeg::{FfmpegError, run_ffmpeg_with_progress};
use super::paths::{output_path, temp_name};
use super::{ErrorKind, Failed, Outcome, PrepareOptions, PrepareResult, settle_report};
use crate::compliance::Audit;
use crate::copy_policy::check_copy_only;
use crate::escalation::{AssessOptions, Assessment, Level, assess};
use crate::fd_fix_contract::{FdFixTarget, UnsafeFixTarget};
use crate::ffprobe::ProbeError;
use crate::models::{Plan, SubAction};
use crate::mp4::patch::{
    apply_moov_patches, check_accessibility, is_faststart, set_subtitle_accessibility,
};
use crate::mp4::tags::{PatchSummary, SubInfo};
use crate::output::{patch_summary_lines, plan_table, verify_atmos_in_output};
use crate::phase_timing::phase;
use crate::pipeline_postfix::{RunContext, finalize_output};
use crate::planner::{PlanOptions, build_plan};
use crate::postfix::bring_into_compliance;
use crate::probe_helpers::{ProbeContext, is_atmos_audio, subtitle_infos};
use crate::stages::Stage;
use crate::verify::{VerifyReport, verify_file};

const MP4_LIKE: [&str; 3] = ["mp4", "m4v", "mov"];

/// What Python lets propagate out of `prepare_file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    /// The caller combined options that cannot work together (Python's `ValueError`).
    Usage(String),
    /// Anything else the run cannot recover from (a malformed MP4, an I/O error, no ffmpeg).
    Other(String),
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for PrepareError {}

/// Why a step ended: a mapped failure (becomes a `failed` result) or an error that propagates.
enum Abort {
    Failed(Failed),
    Propagate(PrepareError),
}

type Step<T> = Result<T, Abort>;

fn failed(kind: ErrorKind, message: impl Into<String>) -> Abort {
    Abort::Failed(Failed {
        kind,
        message: message.into(),
        report: None,
        output: None,
    })
}

impl From<Failed> for Abort {
    fn from(value: Failed) -> Self {
        Self::Failed(value)
    }
}

impl From<UnsafeFixTarget> for Abort {
    fn from(value: UnsafeFixTarget) -> Self {
        failed(ErrorKind::Unsafe, value.0)
    }
}

impl From<FfmpegError> for Abort {
    fn from(value: FfmpegError) -> Self {
        match value {
            FfmpegError::Failed { .. } => failed(ErrorKind::Ffmpeg, value.to_string()),
            FfmpegError::Spawn(_) => Self::Propagate(PrepareError::Other(value.to_string())),
        }
    }
}

impl From<crate::planner::PlanError> for Abort {
    fn from(value: crate::planner::PlanError) -> Self {
        match value {
            // A path ffmpeg cannot be given unchanged: nothing has been written yet.
            crate::planner::PlanError::NonUtf8Path(_) => {
                failed(ErrorKind::Analysis, value.to_string())
            }
            crate::planner::PlanError::NoVideo => {
                Self::Propagate(PrepareError::Other(value.to_string()))
            }
        }
    }
}

macro_rules! propagate {
    ($($error:ty),*) => {$(
        impl From<$error> for Abort {
            fn from(value: $error) -> Self {
                Self::Propagate(PrepareError::Other(value.to_string()))
            }
        }
    )*};
}
propagate!(
    std::io::Error,
    crate::mp4::Mp4Error,
    crate::postfix::PostfixError
);

fn is_mp4_like(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| MP4_LIKE.iter().any(|m| e.eq_ignore_ascii_case(m)))
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

#[allow(clippy::cast_precision_loss)]
fn fraction(done: u64, total: u64) -> f64 {
    done as f64 / total.max(1) as f64
}

fn lines(ctx: &mut RunContext, text: &[String]) {
    for line in text {
        ctx.console.print(line);
    }
}

/// `verify` stage: every rule against `path`, progress reported per rule.
fn verify(ctx: &mut RunContext, path: &Path) -> VerifyReport {
    ctx.reporter.stage(Stage::Verify);
    let _timing = phase("verify", Some(path));
    let reporter = &mut ctx.reporter;
    verify_file(
        path,
        ctx.tools.as_ref(),
        None,
        Some(&mut |done, total| reporter.update(fraction(done as u64, total as u64))),
    )
}

/// `alternate_group`, Apple tags and the Atmos dec3 extension, written to the moov in one go; with
/// `faststart` (an ffmpeg output) the same pass moves the moov in front of the media.
///
/// # Errors
///
/// [`PrepareError::Other`] when the file cannot be patched.
pub fn patch_media(
    ctx: &mut RunContext,
    path: &Path,
    sub_infos: &[SubInfo],
    plan: Option<&Plan>,
    verbose: bool,
    faststart: bool,
) -> Result<(), PrepareError> {
    step_patch_media(ctx, path, sub_infos, plan, verbose, faststart).map_err(into_error)
}

fn into_error(abort: Abort) -> PrepareError {
    match abort {
        Abort::Propagate(error) => error,
        Abort::Failed(failed) => PrepareError::Other(failed.message),
    }
}

fn step_patch_media(
    ctx: &mut RunContext,
    path: &Path,
    sub_infos: &[SubInfo],
    plan: Option<&Plan>,
    verbose: bool,
    faststart: bool,
) -> Step<()> {
    if verbose {
        ctx.console.print(&format!(
            "  → patching alternate_group, elng/chtx for {} subtitle track(s)",
            sub_infos.len()
        ));
    }
    ctx.reporter.stage(Stage::Finalize);
    let (summary, atmos_patched) = {
        let _timing = phase("patch.moov", Some(path));
        let reporter = &mut ctx.reporter;
        apply_moov_patches(
            path,
            sub_infos,
            Some(&mut |done, total| reporter.update(fraction(done, total))),
            faststart,
        )?
    };
    lines(ctx, &patch_summary_lines(&summary, plan));
    if atmos_patched && verbose {
        ctx.console.print("  → patched Atmos dec3 JOC extension");
    }
    Ok(())
}

/// Write the Apple tags into `path`. A manager target must only change through its contract, so it
/// is patched on a temporary copy that `replace_from` swaps in after re-checking the identity.
///
/// # Errors
///
/// [`PrepareError`] when the copy, the patch or the replacement fails.
pub fn patch_tags(
    ctx: &mut RunContext,
    path: &Path,
    sub_infos: &[SubInfo],
    target: Option<&FdFixTarget>,
    progress: Option<&mut dyn FnMut(u64, u64)>,
) -> Result<PatchSummary, PrepareError> {
    let _ = ctx;
    patch_tags_step(path, sub_infos, target, progress).map_err(into_error)
}

fn patch_tags_step(
    path: &Path,
    sub_infos: &[SubInfo],
    target: Option<&FdFixTarget>,
    mut progress: Option<&mut dyn FnMut(u64, u64)>,
) -> Step<PatchSummary> {
    let Some(target) = target else {
        let _timing = phase("patch.tags", Some(path));
        return Ok(set_subtitle_accessibility(path, sub_infos, progress)?);
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(temp_name(&stem_of(path), "tags_"));
    let outcome = (|| -> Step<PatchSummary> {
        let size = std::fs::metadata(path)?.len();
        std::fs::copy(path, &tmp)?;
        if let Some(report) = progress.as_mut() {
            report(size, 2 * size);
        }
        let mut shifted = |done: u64, total: u64| {
            if let Some(report) = progress.as_mut() {
                report(size + done, size + total);
            }
        };
        let summary = set_subtitle_accessibility(&tmp, sub_infos, Some(&mut shifted))?;
        target.replace_from(&name_of(&tmp))?;
        Ok(summary)
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    outcome
}

/// What the run knows while it goes on; `failed` results are built from it.
struct Run<'a> {
    source: &'a Path,
    options: &'a PrepareOptions,
    level: Level,
    reasons: Vec<String>,
    /// The file analysed and named in a failure report: the source or its sibling MP4.
    subject: PathBuf,
    temps: Vec<PathBuf>,
    quarantine: bool,
}

struct Analysis {
    probe: Value,
    ctx: ProbeContext,
    sidecars: Vec<PathBuf>,
    assessment: Assessment,
}

fn analyse(ctx: &mut RunContext, run: &mut Run, path: &Path) -> Step<Analysis> {
    let options = run.options;
    let probe = ctx.ffprobe.probe_json(path).map_err(|error| match error {
        ProbeError::Failed(_) => failed(
            ErrorKind::Analysis,
            format!("could not analyse {}: {error}", name_of(path)),
        ),
        other => failed(ErrorKind::Analysis, other.to_string()),
    })?;
    let probe_ctx = ProbeContext {
        infile: path.to_path_buf(),
        ffprobe: probe.clone(),
        first_video_dts: ctx.ffprobe.first_video_dts(path),
        is_faststart: is_mp4_like(path) && is_faststart(path).unwrap_or(false),
    };
    let sidecars = subtitle_sidecars(path, &options.extra_sidecars, &ctx.ffprobe);
    let audit = Audit::new(ctx.tools.as_ref());
    let mut assessment = assess(
        &audit,
        path,
        &probe,
        &sidecars,
        &probe_ctx,
        &AssessOptions {
            fix_dts: options.fix_dts,
            prefer_hevc10: options.prefer_hevc10,
            keep_av1: options.keep_av1,
            force: options.force,
        },
    );
    if !is_mp4_like(path) && matches!(assessment.level, Level::None | Level::InPlace) {
        // Only a new MP4 can bring another container to the target, never a copy of it.
        let suffix = path.extension().map_or_else(String::new, |e| {
            format!(".{}", e.to_string_lossy().to_lowercase())
        });
        assessment.level = Level::Rebuild;
        assessment.reasons = vec![format!("source is {suffix} (must remux to mp4)")];
        assessment.tags_only = false;
    }
    run.level = assessment.level;
    run.reasons.clone_from(&assessment.reasons);
    Ok(Analysis {
        probe,
        ctx: probe_ctx,
        sidecars,
        assessment,
    })
}

fn plan_for(
    ctx: &RunContext,
    options: &PrepareOptions,
    source: &Path,
    out: &Path,
    analysis: &Analysis,
) -> Step<Plan> {
    Ok(build_plan(
        source,
        &analysis.probe,
        out,
        &analysis.sidecars,
        &PlanOptions {
            prefer_hevc10: options.prefer_hevc10,
            crf: options.crf,
            keep_av1: options.keep_av1,
            fix_dts: options.fix_dts,
            skip_audio: analysis.assessment.duplicates.clone(),
        },
        &analysis.ctx,
        &ctx.ffprobe,
    )?)
}

/// ffmpeg from `source` into `tmp`, then the moov patch and the post-run verify of `tmp`.
fn encode(
    ctx: &mut RunContext,
    options: &PrepareOptions,
    source: &Path,
    analysis: &Analysis,
    tmp: &Path,
) -> Step<VerifyReport> {
    let plan = plan_for(ctx, options, source, tmp, analysis)?;
    let transcode = |error: &dyn fmt::Display| {
        failed(
            ErrorKind::Transcode,
            format!("this fix would need to re-encode media: {error}"),
        )
    };
    if options.copy_only {
        let checked = check_copy_only(&plan.ffmpeg_cmd).and_then(|()| {
            verify_subtitle_inputs(&plan, &analysis.probe, &analysis.sidecars, &ctx.ffprobe)
        });
        if let Err(violation) = checked {
            return Err(transcode(&violation));
        }
    }
    lines(ctx, &plan_table(&plan));
    {
        let _timing = phase("ffmpeg", Some(tmp));
        run_ffmpeg_with_progress(ctx, &plan)?;
    }
    if std::fs::metadata(tmp).map_or(true, |m| !m.is_file() || m.len() == 0) {
        return Err(failed(
            ErrorKind::NoOutput,
            format!("ffmpeg wrote no output for {}", name_of(source)),
        ));
    }
    if options.copy_only {
        // ffmpeg opened the sidecars by name after they were checked. Whatever happened in
        // between, nothing is published unless the file holds exactly the planned streams.
        if let Err(violation) = verify_output_streams(tmp, &plan, &analysis.probe, &ctx.ffprobe) {
            return Err(transcode(&violation));
        }
    }
    let infos: Vec<SubInfo> = plan
        .sub_tracks
        .iter()
        .filter(|t| t.action != SubAction::Skip)
        .map(|t| (t.lang.clone(), t.sdh, t.forced))
        .collect();
    step_patch_media(ctx, tmp, &infos, Some(&plan), options.verbose, true)?;
    let atmos = plan.audio_tracks.iter().filter(|t| t.is_atmos).count();
    for warning in verify_atmos_in_output(tmp, atmos, &ctx.ffprobe) {
        ctx.console.print(&format!("  ! {warning}"));
    }
    Ok(finalize_output(ctx, tmp, atmos > 0, None, options.verbose)?)
}

fn has_atmos(probe: &Value) -> bool {
    probe
        .get("streams")
        .and_then(Value::as_array)
        .is_some_and(|streams| {
            streams.iter().any(|s| {
                s.get("codec_type").and_then(Value::as_str) == Some("audio") && is_atmos_audio(s)
            })
        })
}

/// The in-place steps (postfix, then the tag patch when needed) on a copy of `source` at `tmp`.
fn patch_copy(
    ctx: &mut RunContext,
    source: &Path,
    tmp: &Path,
    probe: &Value,
) -> Step<VerifyReport> {
    let atmos = has_atmos(probe);
    let infos = subtitle_infos(probe);
    ctx.reporter.begin_file(false);
    ctx.reporter.stage(Stage::Finalize);
    {
        let _timing = phase("copy", Some(tmp));
        std::fs::copy(source, tmp)?;
    }
    {
        let _timing = phase("postfix", Some(tmp));
        bring_into_compliance(tmp, atmos, Some(probe), &ctx.ffprobe)?;
    }
    if check_accessibility(tmp, &infos)?.needs_patch {
        let summary = tag_patch(ctx, tmp, &infos)?;
        lines(ctx, &patch_summary_lines(&summary, None));
    }
    Ok(verify(ctx, tmp))
}

fn tag_patch(ctx: &mut RunContext, path: &Path, infos: &[SubInfo]) -> Step<PatchSummary> {
    let _timing = phase("patch.tags", Some(path));
    let reporter = &mut ctx.reporter;
    Ok(set_subtitle_accessibility(
        path,
        infos,
        Some(&mut |done, total| reporter.update(fraction(done, total))),
    )?)
}

fn temp_path(source: &Path, target: &FdFixTarget) -> Step<PathBuf> {
    let name = temp_name(&stem_of(source), "");
    leaf_in(source, target, &name)
}

/// `leaf` in the pinned directory; without a real root the caller's path names the same directory.
fn leaf_in(source: &Path, target: &FdFixTarget, leaf: &str) -> Step<PathBuf> {
    if target.root().is_some() {
        Ok(target.sibling_path(leaf)?)
    } else {
        Ok(source.with_file_name(leaf))
    }
}

fn has_sibling(target: &FdFixTarget, leaf: &str) -> Step<bool> {
    target.has_sibling(leaf).map_err(|_| {
        UnsafeFixTarget(format!("could not check for {leaf} next to the source")).into()
    })
}

/// Postfix and, when needed, the tag patch; through a temporary copy whenever the tags change.
///
/// Patching first and replacing last keeps the pinned identity intact: a postfix on the source
/// would change its ctime and the replacement would then refuse it.
fn patch_in_place(
    ctx: &mut RunContext,
    run: &mut Run,
    target: &FdFixTarget,
    probe: &Value,
) -> Step<VerifyReport> {
    let (source, options) = (run.source, run.options);
    let atmos = has_atmos(probe);
    let infos = subtitle_infos(probe);
    ctx.reporter.begin_file(false);
    ctx.reporter.stage(Stage::Finalize);
    if check_accessibility(source, &infos)?.needs_patch {
        let tmp = temp_path(source, target)?;
        run.temps.push(tmp.clone());
        std::fs::copy(source, &tmp)?;
        {
            let _timing = phase("postfix", Some(&tmp));
            bring_into_compliance(&tmp, atmos, Some(probe), &ctx.ffprobe)?;
        }
        let summary = tag_patch(ctx, &tmp, &infos)?;
        lines(ctx, &patch_summary_lines(&summary, None));
        let report = verify(ctx, &tmp);
        settle_report(
            ctx,
            &name_of(source),
            options,
            &tmp,
            &report,
            run.quarantine,
        )?;
        target.replace_from(&name_of(&tmp))?;
        return Ok(report);
    }
    target.revalidate()?;
    {
        let _timing = phase("postfix", Some(source));
        // Postfix reopens its file several times; through this descriptor every open reaches the pinned
        // inode, not whatever the name points to by then.
        let pinned = target.open_for_write()?;
        let pinned_path = PathBuf::from(format!("/dev/fd/{}", pinned.as_raw_fd()));
        bring_into_compliance(&pinned_path, atmos, Some(probe), &ctx.ffprobe)?;
    }
    let report = verify(ctx, source);
    if !report.passed() {
        // The source is already rewritten; callers report it as changed (never quarantined).
        return Err(Abort::Failed(Failed {
            kind: ErrorKind::Blockers,
            message: format!(
                "{} was repaired in place but {} compliance blocker(s) remain",
                name_of(source),
                report.blockers().len()
            ),
            report: Some(report),
            output: Some(source.to_path_buf()),
        }));
    }
    Ok(report)
}

fn result_of(
    source: &Path,
    assessment: &Assessment,
    outcome: Outcome,
    output: Option<PathBuf>,
    report: Option<VerifyReport>,
) -> PrepareResult {
    PrepareResult {
        source: source.to_path_buf(),
        output,
        level: assessment.level,
        outcome,
        reasons: assessment.reasons.clone(),
        tags_only: assessment.tags_only,
        report,
        source_removed: false,
        source_kept: None,
        error: None,
        error_kind: None,
    }
}

fn rebuilt_or_reencoded(level: Level) -> Outcome {
    if level == Level::Rebuild {
        Outcome::Rebuilt
    } else {
        Outcome::Reencoded
    }
}

fn run_level(
    ctx: &mut RunContext,
    run: &mut Run,
    target: &FdFixTarget,
    analysis: &Analysis,
    source: &Path,
) -> Step<PrepareResult> {
    let options = run.options;
    let assessment = &analysis.assessment;
    let level = assessment.level;
    let finish = |outcome: Outcome, report: Option<VerifyReport>| {
        let changed = matches!(
            outcome,
            Outcome::Patched | Outcome::Rebuilt | Outcome::Reencoded
        );
        result_of(
            source,
            assessment,
            outcome,
            changed.then(|| source.to_path_buf()),
            report,
        )
    };
    if level == Level::None {
        return Ok(finish(Outcome::Done, None));
    }
    if level == Level::Reencode && options.copy_only {
        return Err(failed(
            ErrorKind::Transcode,
            "this fix would need to re-encode the video, which is not allowed here",
        ));
    }
    if level == Level::Reencode && !options.delete_original {
        return Ok(finish(Outcome::ReencodeNeeded, None));
    }
    if options.dry_run {
        if matches!(level, Level::Rebuild | Level::Reencode) {
            let tmp = temp_path(source, target)?;
            let plan = plan_for(ctx, options, source, &tmp, analysis)?;
            lines(ctx, &plan_table(&plan));
        }
        return Ok(finish(Outcome::Planned, None));
    }
    if level == Level::InPlace {
        let report = patch_in_place(ctx, run, target, &analysis.probe)?;
        return Ok(finish(Outcome::Patched, Some(report)));
    }
    let tmp = temp_path(source, target)?;
    run.temps.push(tmp.clone());
    let report = encode(ctx, options, source, analysis, &tmp)?;
    settle_report(
        ctx,
        &name_of(source),
        options,
        &tmp,
        &report,
        run.quarantine,
    )?;
    target.replace_from(&name_of(&tmp))?;
    Ok(finish(rebuilt_or_reencoded(level), Some(report)))
}

fn run_new_sibling(
    ctx: &mut RunContext,
    run: &mut Run,
    target: &FdFixTarget,
    analysis: &Analysis,
    final_leaf: &str,
) -> Step<PrepareResult> {
    let (source, options) = (run.source, run.options);
    let assessment = &analysis.assessment;
    let level = assessment.level;
    let final_path = leaf_in(source, target, final_leaf)?;
    let tmp = temp_path(source, target)?;
    if options.dry_run {
        let plan = plan_for(ctx, options, source, &tmp, analysis)?;
        lines(ctx, &plan_table(&plan));
        return Ok(result_of(source, assessment, Outcome::Planned, None, None));
    }
    let started_ns = i128::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    )
    .unwrap_or(i128::MAX);
    run.temps.push(tmp.clone());
    let report = encode(ctx, options, source, analysis, &tmp)?;
    settle_report(ctx, final_leaf, options, &tmp, &report, run.quarantine)?;
    target.publish_new(&name_of(&tmp), final_leaf)?;
    let passed = report.passed();
    let mut done = result_of(
        source,
        assessment,
        rebuilt_or_reencoded(level),
        Some(final_path),
        Some(report),
    );
    if options.delete_original && passed {
        if has_sibling(target, target.leaf_name())? {
            done.source_kept = target.remove_after_output(final_leaf, started_ns);
            done.source_removed = done.source_kept.is_none();
        } else {
            done.source_kept = Some("the original was already gone".to_owned());
        }
    }
    Ok(done)
}

fn run_to_output(
    ctx: &mut RunContext,
    run: &mut Run,
    analysis: &Analysis,
    output: &Path,
) -> Step<PrepareResult> {
    let (source, options) = (run.source, run.options);
    let assessment = &analysis.assessment;
    let level = assessment.level;
    let tmp = output.with_file_name(temp_name(&stem_of(output), ""));
    if options.dry_run {
        if matches!(level, Level::Rebuild | Level::Reencode) {
            let plan = plan_for(ctx, options, source, &tmp, analysis)?;
            lines(ctx, &plan_table(&plan));
        }
        return Ok(result_of(source, assessment, Outcome::Planned, None, None));
    }
    if output.is_dir() {
        return Err(UnsafeFixTarget("the output path is a directory".to_owned()).into());
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| {
            Abort::from(UnsafeFixTarget(format!(
                "could not create the output directory: {error}"
            )))
        })?;
    }
    run.temps.push(tmp.clone());
    let (report, outcome) = if matches!(level, Level::None | Level::InPlace) {
        let report = patch_copy(ctx, source, &tmp, &analysis.probe)?;
        (
            report,
            if level == Level::None {
                Outcome::Done
            } else {
                Outcome::Patched
            },
        )
    } else {
        let report = encode(ctx, options, source, analysis, &tmp)?;
        (report, rebuilt_or_reencoded(level))
    };
    settle_report(
        ctx,
        &name_of(output),
        options,
        &tmp,
        &report,
        run.quarantine,
    )?;
    std::fs::rename(&tmp, output).map_err(|error| {
        Abort::from(UnsafeFixTarget(format!(
            "could not move the output into place: {error}"
        )))
    })?;
    Ok(result_of(
        source,
        assessment,
        outcome,
        Some(output.to_path_buf()),
        Some(report),
    ))
}

/// A caller target without a real root must pin the directory `source` lives in.
fn check_source_in_target(source: &Path, target: &FdFixTarget) -> Step<()> {
    if target.root().is_some() || source == target.path() {
        return Ok(());
    }
    let parent = source.parent().filter(|p| !p.as_os_str().is_empty());
    let meta = std::fs::metadata(parent.unwrap_or_else(|| Path::new("."))).map_err(|_| {
        Abort::from(UnsafeFixTarget(
            "could not check the source directory".to_owned(),
        ))
    })?;
    {
        use std::os::unix::fs::MetadataExt;
        if (i128::from(meta.dev()), i128::from(meta.ino())) != target.parent_identity() {
            return Err(
                UnsafeFixTarget("the source is not in the pinned directory".to_owned()).into(),
            );
        }
    }
    Ok(())
}

fn with_source_report(report: Option<VerifyReport>, source: &Path) -> Option<VerifyReport> {
    report.map(|mut r| {
        if r.path != source {
            r.path = source.to_path_buf();
        }
        r
    })
}

/// The route decision and its run; the caller cleans up.
fn route(
    ctx: &mut RunContext,
    run: &mut Run,
    target: &FdFixTarget,
    output: Option<&Path>,
) -> Step<PrepareResult> {
    let (source, options) = (run.source, run.options);
    let sibling_leaf = format!("{}.mp4", stem_of(source));
    if let Some(output) = output {
        let analysis = analyse(ctx, run, source)?;
        return run_to_output(ctx, run, &analysis, output);
    }
    if is_mp4_like(source) {
        let analysis = analyse(ctx, run, source)?;
        return run_level(ctx, run, target, &analysis, source);
    }
    if has_sibling(target, &sibling_leaf)? {
        // The MP4 next to the source is what the pipeline works on; the source stays as it is.
        let sibling_target = target.sibling(&sibling_leaf)?;
        let subject = leaf_in(source, target, &sibling_leaf)?;
        run.subject.clone_from(&subject);
        let analysis = analyse(ctx, run, &subject)?;
        let mut result = run_level(ctx, run, &sibling_target, &analysis, &subject)?;
        result.source = source.to_path_buf();
        result.output = Some(subject);
        if options.delete_original {
            result.source_kept =
                Some("the original was kept: an existing MP4 next to it was used".to_owned());
        }
        return Ok(result);
    }
    let analysis = analyse(ctx, run, source)?;
    run_new_sibling(ctx, run, target, &analysis, &sibling_leaf)
}

/// Bring `source` to the Apple TV target, going no further than its escalation level requires.
///
/// `target` pins the source for a caller that already holds its directory (the manager); without
/// it the file is pinned here and released again, and only then may a failing output go to
/// `quarantine_dir`.
///
/// Where the result goes: an MP4-like source without an output is patched or rebuilt in place
/// through its target; another container becomes the sibling `<stem>.mp4` (an existing sibling is
/// the file the pipeline works on and the source stays untouched; a new one is never published
/// over an existing file, and `delete_original` removes the source only after a verified output);
/// `options.output` (a file, or a directory for `<dir>/<stem>.mp4`) never modifies the input.
///
/// # Errors
///
/// [`PrepareError::Usage`] for a quarantine directory together with a caller target, or
/// `delete_original` together with an output; [`PrepareError::Other`] for what Python lets
/// propagate. Every mapped failure is a `failed` [`PrepareResult`] instead, with no temporary file
/// left behind.
pub fn prepare_file(
    ctx: &mut RunContext,
    source: &Path,
    options: &PrepareOptions,
    target: Option<&FdFixTarget>,
) -> Result<PrepareResult, PrepareError> {
    if target.is_some() && options.quarantine_dir.is_some() {
        return Err(PrepareError::Usage(
            "a caller-supplied fix target does not permit quarantine moves".to_owned(),
        ));
    }
    if options.delete_original && options.output.is_some() {
        return Err(PrepareError::Usage(
            "--delete-original cannot be combined with an output target".to_owned(),
        ));
    }
    let output = output_path(source, options.output.as_deref());
    let mut run = Run {
        source,
        options,
        level: Level::None,
        reasons: Vec::new(),
        subject: source.to_path_buf(),
        temps: Vec::new(),
        quarantine: target.is_none(),
    };
    let outcome = pinned(ctx, &mut run, source, target, output.as_deref());
    for tmp in &run.temps {
        let _ = std::fs::remove_file(tmp);
    }
    match outcome {
        Ok(mut result) => {
            let shown = result
                .output
                .clone()
                .unwrap_or_else(|| source.to_path_buf());
            result.report = with_source_report(result.report.take(), &shown);
            Ok(result)
        }
        Err(Abort::Failed(error)) => Ok(PrepareResult {
            source: source.to_path_buf(),
            output: error.output,
            level: run.level,
            outcome: Outcome::Failed,
            reasons: run.reasons,
            tags_only: false,
            report: with_source_report(error.report, &run.subject),
            source_removed: false,
            source_kept: None,
            error: Some(if error.message.is_empty() {
                "Error".to_owned()
            } else {
                error.message
            }),
            error_kind: Some(error.kind),
        }),
        Err(Abort::Propagate(error)) => Err(error),
    }
}

fn pinned(
    ctx: &mut RunContext,
    run: &mut Run,
    source: &Path,
    target: Option<&FdFixTarget>,
    output: Option<&Path>,
) -> Step<PrepareResult> {
    let opened;
    let target = if let Some(target) = target {
        check_source_in_target(source, target)?;
        target
    } else {
        opened = FdFixTarget::for_path(source)?;
        &opened
    };
    if target.leaf_name() != name_of(source) {
        return Err(UnsafeFixTarget("the pinned leaf is not the source file".to_owned()).into());
    }
    route(ctx, run, target, output)
}

//! Execute claimed inspection and verification items under leases and gate permits.
use super::process::{
    PrepareMode, ProcessResult, StopReason, run_pinned_prepare_observed, run_pinned_probe,
    run_pinned_verify,
};
use crate::{
    Error, Result,
    deployment_gate::DeploymentGate,
    jobs::Jobs,
    leases::MediaExecutorLease,
    repo::Ctx,
    settings::{TargetProfile, target_profile},
    snapshots::{InputSnapshot, InputSnapshots},
    state_machine::{ItemStatus, JobStateMachine, JobStatus},
};
use atvprep_core::encode::{EncodeAllowance, EncodeNeed};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

/// How long an item that may encode video and needs it (or does not know yet) can run. Encoding a
/// film takes hours; the lease keeps being renewed meanwhile. Every other item keeps the configured,
/// shorter limit.
pub const REENCODE_TIMEOUT: Duration = Duration::from_hours(4);

/// The code of a successful item whose file needs more than its Kodierfreigabe (ADR 0008).
pub const ENCODE_NOT_ALLOWED: &str = "encode_not_allowed";

/// The machine line the media CLI prints when a file needs more than its Kodierfreigabe allows.
const ENCODE_NOT_ALLOWED_LINE: &str = "ATVPREP-OUTCOME encode_not_allowed";

/// The need and allowance of a refusal, from the CLI's `ATVPREP-ENCODE` line.
fn encode_refusal(process: &ProcessResult) -> Option<String> {
    if process.reason != StopReason::Exited || process.exit_code != Some(0) {
        return None;
    }
    // Machine lines come on stdout alone; the merged log may interleave stderr with them.
    let output = String::from_utf8_lossy(&process.stdout);
    output
        .lines()
        .any(|line| line.trim() == ENCODE_NOT_ALLOWED_LINE)
        .then(|| {
            let detail = output
                .lines()
                .find_map(|line| line.trim().strip_prefix("ATVPREP-ENCODE "))
                .unwrap_or("need=unknown allowance=unknown");
            format!("Encoding not allowed ({detail}): the file stays as it was.")
        })
}

/// The time limit of one changing item: longer only for „Audio und Video“ with a video or unknown
/// Kodierbedarf (`need` is the stored one, `None` is „unbekannt“).
#[must_use]
pub fn prepare_timeout(
    configured: Duration,
    mode: PrepareMode,
    need: Option<EncodeNeed>,
) -> Duration {
    if mode.encode == EncodeAllowance::All && matches!(need, None | Some(EncodeNeed::Video)) {
        configured.max(REENCODE_TIMEOUT)
    } else {
        configured
    }
}

/// Trusted runtime settings for the media CLI.
#[derive(Clone)]
pub struct ExecutionConfig {
    pub executable: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub timeout: Duration,
    pub grace: Duration,
    pub lease_seconds: i64,
    pub shutdown: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub approvals: Option<std::sync::Arc<crate::approval::ApprovalVerifier>>,
    pub session_idle_seconds: i64,
}

fn authority(ctx: Ctx<'_>, item: i64, owner: &str, fence: i64) -> Result<bool> {
    crate::db::retry_busy(|| {
        Ok(ctx.conn.query_row("SELECT EXISTS(SELECT 1 FROM job_items i JOIN media_executor_leases l ON l.id=1 WHERE i.id=?1 AND i.claimed_by=?2 AND i.executor_fencing_token=?3 AND i.lease_expires_at>?4 AND l.owner_id=?2 AND l.fencing_token=?3 AND l.lease_expires_at>?4)",rusqlite::params![item,owner,fence,ctx.clock.now()],|row|row.get(0))?)
    })
}

/// Run one claimed inspection/verification item, renewing leases and observing cancellation.
///
/// # Errors
/// Rejects stale claims, changed inputs, closed gates and subprocess failures.
#[allow(clippy::too_many_lines)] // Keep permit cleanup and the fenced process lifecycle together.
pub fn execute_claimed(
    ctx: Ctx<'_>,
    item_id: i64,
    owner: &str,
    fence: i64,
    config: &ExecutionConfig,
) -> Result<ProcessResult> {
    if config.lease_seconds < 3 || !authority(ctx, item_id, owner, fence)? {
        return Err(Error::Conflict(
            "worker claim is no longer current".to_owned(),
        ));
    }
    let item = crate::db::retry_busy(|| Jobs(ctx).get_item(item_id))?
        .ok_or_else(|| Error::Invalid("job item not found".to_owned()))?;
    if !matches!(
        item.action.as_str(),
        "verify" | "inspect" | "fix" | "convert"
    ) || item.status != ItemStatus::Claimed
    {
        return Err(Error::Invalid(
            "item is not a claimed verification".to_owned(),
        ));
    }
    let parent = crate::db::retry_busy(|| Jobs(ctx).get(item.job_id))?
        .ok_or_else(|| Error::Invalid("parent job not found".to_owned()))?;
    if !matches!(parent.status, JobStatus::Queued | JobStatus::Running) {
        return Err(Error::Conflict(
            "parent job no longer accepts execution".to_owned(),
        ));
    }
    // Everything that decides whether this input may still run happens before any permit is taken. A
    // definite "no" (file gone or changed, approval stale) ends the item at once instead of leaving it
    // claimed until its lease runs out and recovery queues the same doomed work again.
    let checked = (|| -> Result<_> {
        let (root, relative): (String, String) = crate::db::retry_busy(|| {
            Ok(ctx.conn.query_row("SELECT r.canonical_path,m.relative_path FROM media_files m JOIN library_roots r ON r.id=m.root_id WHERE m.id=?1 AND m.discovery_state='present'",[item.media_file_id],|row|Ok((row.get(0)?,row.get(1)?)))?)
        })?;
        let target = atvprep_core::fd_fix_contract::FdFixTarget::from_library(
            std::path::Path::new(&root),
            &relative,
        )
        .map_err(|_| Error::Invalid("cannot safely open verification input".to_owned()))?;
        let recorded = crate::db::retry_busy(|| InputSnapshots(ctx).get_for_item(item_id))?
            .ok_or_else(|| Error::Invalid("verification has no recorded input".to_owned()))?;
        let recorded = InputSnapshot::from_json(&recorded)?;
        if target.identity()
            != (
                i128::from(recorded.file_device),
                i128::from(recorded.file_inode),
                i128::from(recorded.file_ctime_ns),
            )
        {
            return Err(Error::Invalid("verification input changed".to_owned()));
        }
        let changing = matches!(item.action.as_str(), "fix" | "convert");
        let mut mode = PrepareMode {
            encode: EncodeAllowance::Copy,
            delete_original: false,
            force: false,
            profile: TargetProfile::default(),
        };
        if changing {
            // A job from an older policy can never become valid: refused for good, "queue it again".
            if let Some(reason) = crate::approval::prepare_policy_block(&item.settings_snapshot) {
                return Err(Error::Invalid(reason.to_owned()));
            }
            // Signed for the profile it was queued under: a changed profile is a new decision.
            if crate::approval::item_profile(&item.settings_snapshot)
                != Some(crate::db::retry_busy(|| target_profile(ctx.conn))?)
            {
                return Err(Error::Invalid(
                    "The target profile changed after this job was queued. Queue the preparation again."
                        .to_owned(),
                ));
            }
            let verifier = config
                .approvals
                .as_ref()
                .ok_or_else(|| Error::Invalid("manual approvals are not configured".to_owned()))?;
            let current = super::scanner::observe(
                std::path::Path::new(&root),
                &relative,
                0,
                ctx.clock.now_micros(),
            )?;
            let sidecar_fingerprint = current
                .sidecars
                .iter()
                .map(|sidecar| {
                    format!(
                        "{}:{}",
                        sidecar.relative_path,
                        super::scanner::fingerprint(sidecar.size, sidecar.mtime_ns)
                    )
                })
                .collect::<Vec<_>>()
                .join("|");
            let identity = [
                serde_json::json!(current.meta.scanned_device),
                serde_json::json!(current.meta.scanned_inode),
                serde_json::json!(current.meta.scanned_ctime_ns),
                serde_json::json!(sidecar_fingerprint),
            ];
            // A manual approval that does not hold right now (revoked session, idle timeout) may hold
            // again: it is reported as a conflict, so the claim stays and is not ended as stale input.
            // A schedule's refusal (paused, gone, changed since queueing, bad signature) is permanent:
            // it ends the item as stale input with the reason, so claim and lease do not linger.
            let from_schedule = item
                .settings_snapshot
                .get("schedule_id")
                .is_some_and(|id| !id.is_null());
            mode = super::manual_policy::authorize_prepare(
                ctx,
                item_id,
                verifier,
                &identity,
                config.session_idle_seconds,
            )
            .map_err(|error| {
                if from_schedule {
                    error
                } else {
                    Error::Conflict(error.to_string())
                }
            })?;
            target.revalidate().map_err(|_| {
                Error::Invalid("approved input changed before execution".to_owned())
            })?;
        }
        Ok((root, relative, target, recorded, changing, mode))
    })();
    let (root, relative, target, recorded, changing, mode) = match checked {
        Ok(checked) => checked,
        Err(error) => {
            if !matches!(error, Error::Conflict(_))
                && authority(ctx, item_id, owner, fence).unwrap_or(false)
                && JobStateMachine(ctx)
                    .transition_item(
                        item_id,
                        ItemStatus::StaleInput,
                        item.status_version,
                        Some(fence),
                        Some(&serde_json::json!({"error": error.to_string()})),
                    )
                    .is_ok()
            {
                let _ = ctx.conn.execute(
                    "UPDATE job_items SET error_detail=?1 WHERE id=?2",
                    rusqlite::params![error.to_string(), item_id],
                );
                let _ = super::recovery::Recovery(ctx).reconcile_idle_jobs();
            }
            return Err(error);
        }
    };
    let gate = DeploymentGate(ctx);
    let permit = gate.acquire_permit("job", owner)?.ok_or_else(|| {
        Error::Conflict("maintenance: new work is not accepted right now".to_owned())
    })?;
    let result = (|| {
        if !authority(ctx, item_id, owner, fence)? {
            return Err(Error::Conflict(
                "worker claim is no longer current".to_owned(),
            ));
        }
        JobStateMachine(ctx).transition_item(
            item_id,
            ItemStatus::Running,
            item.status_version,
            Some(fence),
            None,
        )?;
        let mut heartbeat = Instant::now();
        let mut lost = false;
        let mut shutdown_requested = false;
        let mut cancelled = || {
            if config
                .shutdown
                .as_ref()
                .is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Acquire))
            {
                shutdown_requested = true;
                return true;
            }
            if heartbeat.elapsed() >= Duration::from_secs(1) {
                heartbeat = Instant::now();
                if !MediaExecutorLease::new(ctx)
                    .heartbeat(Some(fence), config.lease_seconds)
                    .unwrap_or(false)
                    || !Jobs(ctx)
                        .heartbeat_item(item_id, fence, config.lease_seconds)
                        .unwrap_or(false)
                {
                    lost = true;
                    return true;
                }
            }
            if !authority(ctx, item_id, owner, fence).unwrap_or(false) {
                lost = true;
                return true;
            }
            let status: std::result::Result<JobStatus, _> = ctx.conn.query_row(
                "SELECT status FROM jobs WHERE id=?1",
                [item.job_id],
                |row| row.get(0),
            );
            status.map_or(true, |status| {
                !matches!(status, JobStatus::Queued | JobStatus::Running)
            })
        };
        // „Prüfen“ runs against the profile of this moment; what it stored is checked again below.
        let verify_profile = crate::db::retry_busy(|| target_profile(ctx.conn))?;
        let mut output =
            super::execution_output::ClaimedOutput::new(ctx, owner, fence, item_id, item.job_id);
        let process = if changing {
            let need = crate::media::Media(ctx)
                .get(item.media_file_id)?
                .and_then(|media| media.encode_need);
            run_pinned_prepare_observed(
                &config.executable,
                &target,
                mode,
                &config.environment,
                prepare_timeout(config.timeout, mode, need),
                config.grace,
                &mut cancelled,
                |chunk| output.feed(chunk, false),
            )
        } else if item.action == "inspect" {
            run_pinned_probe(
                &config.executable,
                &target,
                &config.environment,
                config.timeout,
                config.grace,
                &mut cancelled,
            )
        } else {
            run_pinned_verify(
                &config.executable,
                &target,
                verify_profile,
                &config.environment,
                config.timeout,
                config.grace,
                &mut cancelled,
            )
        }?;
        if changing {
            output.feed(&[], true)?;
        }
        if lost || !authority(ctx, item_id, owner, fence)? {
            return Err(Error::Conflict(
                "worker lost its execution lease".to_owned(),
            ));
        }
        if !changing
            && process.reason == StopReason::Exited
            && (process.exit_code == Some(0)
                || (item.action == "verify" && process.exit_code == Some(1)))
        {
            target
                .revalidate()
                .map_err(|_| Error::Invalid("input changed during inspection".to_owned()))?;
            if process.stdout.len() > 56 * 1024 {
                return Err(Error::Invalid(
                    "inspection report exceeds supported size".to_owned(),
                ));
            }
            let document: serde_json::Value = serde_json::from_slice(&process.stdout)
                .map_err(|_| Error::Invalid("inspection returned an invalid report".to_owned()))?;
            let report = if item.action == "verify" {
                document
                    .get("reports")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|reports| reports.first())
                    .ok_or_else(|| Error::Invalid("verification returned no report".to_owned()))?
            } else {
                &document
            };
            let tx = rusqlite::Transaction::new_unchecked(
                ctx.conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let write = Ctx {
                conn: &tx,
                clock: ctx.clock,
            };
            if !authority(write, item_id, owner, fence)? {
                return Err(Error::Conflict(
                    "worker lost its inspection authority".to_owned(),
                ));
            }
            crate::inspections::Inspections(write).record(
                item.media_file_id,
                &crate::inspections::NewInspection {
                    source_fingerprint: &recorded.file_fingerprint,
                    input_fingerprint: &recorded.file_fingerprint,
                    tool_name: if item.action == "verify" {
                        "verify"
                    } else {
                        "ffprobe"
                    },
                    tool_version: "unknown",
                    result: report,
                },
                None,
            )?;
            if item.action == "verify" {
                forget_need_of_a_superseded_profile(write, item.media_file_id, verify_profile)?;
            }
            tx.commit()?;
        }
        if changing {
            complete_prepare(
                ctx, &item, &target, &root, &relative, owner, fence, &process,
            )?;
        }
        // A verify that reported violations (exit 1) still produced a valid report: the check ran.
        let verified_with_findings =
            item.action == "verify" && !changing && process.exit_code == Some(1);
        let fresh = Jobs(ctx)
            .get_item(item_id)?
            .ok_or_else(|| Error::Invalid("job item not found".to_owned()))?;
        let terminal = if shutdown_requested {
            ItemStatus::Interrupted
        } else if process.reason == StopReason::Cancelled {
            ItemStatus::Cancelled
        } else if process.reason == StopReason::Exited
            && (process.exit_code == Some(0) || verified_with_findings)
        {
            ItemStatus::Succeeded
        } else {
            ItemStatus::Failed
        };
        let version = if fresh.status == ItemStatus::CancelRequested {
            JobStateMachine(ctx)
                .transition_item(
                    item_id,
                    ItemStatus::Cancelling,
                    fresh.status_version,
                    Some(fence),
                    None,
                )?
                .status_version
        } else {
            fresh.status_version
        };
        JobStateMachine(ctx).transition_item(item_id, terminal, version, Some(fence), None)?;
        // A file above its Kodierfreigabe stays unchanged: a success with a hint, not a failure.
        let refusal = changing.then(|| encode_refusal(&process)).flatten();
        let code = match process.reason {
            StopReason::Exited if refusal.is_some() => Some(ENCODE_NOT_ALLOWED),
            StopReason::TimedOut => Some("timeout"),
            StopReason::Cancelled => Some("cancelled"),
            StopReason::Exited if process.exit_code != Some(0) && !verified_with_findings => {
                Some("verification_failed")
            }
            StopReason::Exited => None,
        };
        let changed=ctx.conn.execute("UPDATE job_items SET exit_code=?1,error_code=?2 WHERE id=?3 AND executor_fencing_token=?4 AND lease_expires_at>?5 AND EXISTS(SELECT 1 FROM media_executor_leases WHERE id=1 AND fencing_token=?4 AND lease_expires_at>?5)",rusqlite::params![process.exit_code,code,item_id,fence,ctx.clock.now()])?;
        if changed != 1 {
            return Err(Error::Conflict(
                "worker lost its completion authority".to_owned(),
            ));
        }
        if let Some(refusal) = refusal {
            ctx.conn.execute(
                "UPDATE job_items SET error_detail=?1 WHERE id=?2",
                rusqlite::params![refusal, item_id],
            )?;
        }
        super::recovery::Recovery(ctx).reconcile_idle_jobs()?;
        Ok(process)
    })();
    match result {
        Ok(process) => {
            gate.release_permit(&permit)?;
            Ok(process)
        }
        Err(error) => {
            // The permit must go back whatever the cleanup does, or the gate never quiesces.
            let cleanup = (|| -> Result<()> {
                if authority(ctx, item_id, owner, fence).unwrap_or(false)
                    && let Ok(Some(fresh)) = Jobs(ctx).get_item(item_id)
                    && fresh.status == ItemStatus::Running
                {
                    JobStateMachine(ctx).transition_item(
                        item_id,
                        ItemStatus::Failed,
                        fresh.status_version,
                        Some(fence),
                        None,
                    )?;
                    super::recovery::Recovery(ctx).reconcile_idle_jobs()?;
                }
                Ok(())
            })();
            gate.release_permit(&permit)?;
            cleanup?;
            Err(error)
        }
    }
}

/// A check that ran against a profile changed in the meantime must not leave its need behind: the
/// file goes back to „unbekannt“, and the background check asks again under the new profile.
fn forget_need_of_a_superseded_profile(
    ctx: Ctx<'_>,
    media_id: i64,
    used: TargetProfile,
) -> Result<()> {
    if target_profile(ctx.conn)? != used {
        ctx.conn.execute(
            "UPDATE media_files SET encode_need='unknown', planned_change='unknown' WHERE id=?1",
            [media_id],
        )?;
    }
    Ok(())
}

/// Whether the CLI reported that it removed the old container after a verified MP4.
fn source_removed(process: &ProcessResult) -> bool {
    process.reason == StopReason::Exited
        && process.exit_code == Some(0)
        && String::from_utf8_lossy(&process.stdout)
            .lines()
            .any(|line| line.trim() == "ATVPREP-SOURCE removed")
}

/// The old container is gone on purpose: its row leaves the library (jobs and inspections are
/// kept), so it is not offered as a missing file, and a scan picks up the new MP4.
fn complete_removed_source(
    ctx: Ctx<'_>,
    item: &crate::models::JobItem,
    owner: &str,
    fence: i64,
) -> Result<()> {
    let tx =
        rusqlite::Transaction::new_unchecked(ctx.conn, rusqlite::TransactionBehavior::Immediate)?;
    let write = Ctx {
        conn: &tx,
        clock: ctx.clock,
    };
    if !authority(write, item.id, owner, fence)? {
        return Err(Error::Conflict(
            "worker lost its Prepare completion authority".to_owned(),
        ));
    }
    let media = crate::media::Media(write)
        .get(item.media_file_id)?
        .ok_or_else(|| Error::Invalid("prepared media not found".to_owned()))?;
    tx.execute(
        "UPDATE media_files SET discovery_state='missing',dismissed_at=?1 WHERE id=?2",
        rusqlite::params![ctx.clock.now(), media.id],
    )?;
    crate::scans::Scans(write).request_manual(media.root_id)?;
    tx.commit()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Completion is bound to the claimed item and its pinned input.
fn complete_prepare(
    ctx: Ctx<'_>,
    item: &crate::models::JobItem,
    target: &atvprep_core::fd_fix_contract::FdFixTarget,
    root: &str,
    relative: &str,
    owner: &str,
    fence: i64,
    process: &ProcessResult,
) -> Result<()> {
    if source_removed(process) {
        return complete_removed_source(ctx, item, owner, fence);
    }
    let root = std::path::Path::new(root);
    let after = atvprep_core::fd_fix_contract::FdFixTarget::from_library(root, relative)
        .map_err(|_| Error::Invalid("prepared source is no longer accessible".to_owned()))?;
    if after.parent_identity() != target.parent_identity() {
        return Err(Error::Invalid(
            "prepared source directory changed".to_owned(),
        ));
    }
    let observed = super::scanner::observe(root, relative, 0, ctx.clock.now_micros())?;
    let identity = (
        observed.meta.scanned_device.map(i128::from),
        observed.meta.scanned_inode.map(i128::from),
        observed.meta.scanned_ctime_ns.map(i128::from),
    );
    let current = after.identity();
    if identity != (Some(current.0), Some(current.1), Some(current.2)) {
        return Err(Error::Invalid(
            "prepared source changed during completion".to_owned(),
        ));
    }
    let tx =
        rusqlite::Transaction::new_unchecked(ctx.conn, rusqlite::TransactionBehavior::Immediate)?;
    let write = Ctx {
        conn: &tx,
        clock: ctx.clock,
    };
    if !authority(write, item.id, owner, fence)? {
        return Err(Error::Conflict(
            "worker lost its Prepare completion authority".to_owned(),
        ));
    }
    let media = crate::media::Media(write)
        .get(item.media_file_id)?
        .ok_or_else(|| Error::Invalid("prepared media not found".to_owned()))?;
    let changed = after.identity() != target.identity()
        || observed.meta.size != media.size
        || observed.meta.mtime_ns != media.mtime_ns;
    if changed {
        crate::media::Media(write).upsert(media.root_id, relative, &observed.meta, None)?;
        tx.execute(
            "UPDATE media_files SET compliance_state='unknown',compliance_reasons=?1,encode_need='unknown',planned_change='unknown' WHERE id=?2",
            rusqlite::params![
                "[\"Changed by Prepare; verification is required.\"]",
                media.id
            ],
        )?;
        if process.reason == StopReason::Exited
            && process.exit_code == Some(0)
            && matches!(observed.meta.suffix.as_str(), ".mp4" | ".m4v" | ".mov")
        {
            Jobs(write).create(&crate::jobs::NewJob::new("verify", &[media.id]))?;
        }
    }
    if !matches!(observed.meta.suffix.as_str(), ".mp4" | ".m4v" | ".mov") {
        crate::scans::Scans(write).request_manual(media.root_id)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{clock::FixedClock, db::Db, schema, settings::Settings};
    use serde_json::json;

    /// Runs `script` with the system shell, as the worker runs the media CLI.
    fn shell(script: &str) -> ProcessResult {
        let executable = std::path::Path::new("/bin/sh").canonicalize().unwrap();
        super::super::process::run(
            &executable,
            &["-c".to_owned(), script.to_owned()],
            &std::collections::BTreeMap::new(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(20),
            || false,
        )
        .unwrap()
    }

    /// The CLI writes its machine lines to stdout; stderr is shared with ffmpeg and the tools. A
    /// stderr chunk read while a line is still open must not break the line.
    #[test]
    fn machine_lines_are_read_from_stdout_even_when_stderr_lands_inside_them() {
        let removed = shell(
            "printf 'ATVPREP-SOURCE removed'; sleep 0.2; printf 'ffmpeg warning\\n' >&2; sleep 0.2; printf '\\n'",
        );
        assert!(source_removed(&removed));
        let refused = shell(
            "printf 'ATVPREP-ENCODE need=video allowance=audio\\nATVPREP-OUTCOME encode_not_allowed'; sleep 0.2; printf 'warning\\n' >&2; sleep 0.2; printf '\\n'",
        );
        assert_eq!(
            encode_refusal(&refused).as_deref(),
            Some("Encoding not allowed (need=video allowance=audio): the file stays as it was.")
        );
    }

    #[test]
    fn a_check_against_a_superseded_profile_leaves_the_file_unknown() {
        let db = Db::memory().unwrap();
        let mut conn = db.conn().unwrap();
        schema::migrate(&mut conn).unwrap();
        let clock = FixedClock(1_700_000_000_000_000);
        let ctx = Ctx {
            conn: conn.raw(),
            clock: &clock,
        };
        conn.raw().execute_batch(
            "INSERT INTO library_roots(id,name,canonical_path) VALUES(1,'r','/r');
             INSERT INTO media_files(id,root_id,relative_path,kind,suffix,size,mtime_ns,fingerprint,
               discovery_state,compliance_state,workflow_state,desired_action,encode_need,planned_change)
             VALUES (1,1,'a.mp4','movie','.mp4',1,1,'a','present','clean','idle','none','video','reencode');",
        ).unwrap();
        let need = || {
            conn.raw()
                .query_row("SELECT encode_need FROM media_files WHERE id=1", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap()
        };

        forget_need_of_a_superseded_profile(ctx, 1, TargetProfile::default()).unwrap();
        assert_eq!(
            need(),
            "video",
            "the profile the check ran against still holds"
        );

        Settings(ctx)
            .set("target_crf", &json!(24), "o", None)
            .unwrap();
        conn.raw()
            .execute("UPDATE media_files SET encode_need='video'", [])
            .unwrap();
        forget_need_of_a_superseded_profile(ctx, 1, TargetProfile::default()).unwrap();
        assert_eq!(need(), "unknown");
    }
}

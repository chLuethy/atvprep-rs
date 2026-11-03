//! Execute claimed inspection and verification items under leases and gate permits.
use super::process::{
    ProcessResult, StopReason, run_pinned_prepare_observed, run_pinned_probe, run_pinned_verify,
};
use crate::{
    Error, Result,
    deployment_gate::DeploymentGate,
    jobs::Jobs,
    leases::MediaExecutorLease,
    repo::Ctx,
    snapshots::{InputSnapshot, InputSnapshots},
    state_machine::JobStateMachine,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

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
    ) || item.status != "claimed"
    {
        return Err(Error::Invalid(
            "item is not a claimed verification".to_owned(),
        ));
    }
    let parent = crate::db::retry_busy(|| Jobs(ctx).get(item.job_id))?
        .ok_or_else(|| Error::Invalid("parent job not found".to_owned()))?;
    if !matches!(parent.status.as_str(), "queued" | "running") {
        return Err(Error::Conflict(
            "parent job no longer accepts execution".to_owned(),
        ));
    }
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
    if changing {
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
        super::manual_policy::authorize_prepare(
            ctx,
            item_id,
            verifier,
            &identity,
            config.session_idle_seconds,
        )?;
        target
            .revalidate()
            .map_err(|_| Error::Invalid("approved input changed before execution".to_owned()))?;
    }
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
            "running",
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
            let status: std::result::Result<String, _> = ctx.conn.query_row(
                "SELECT status FROM jobs WHERE id=?1",
                [item.job_id],
                |row| row.get(0),
            );
            status.map_or(true, |status| {
                !matches!(status.as_str(), "queued" | "running")
            })
        };
        let mut output =
            super::execution_output::ClaimedOutput::new(ctx, owner, fence, item_id, item.job_id);
        let process = if changing {
            run_pinned_prepare_observed(
                &config.executable,
                &target,
                &config.environment,
                config.timeout,
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
            if process.output.len() > 56 * 1024 {
                return Err(Error::Invalid(
                    "inspection report exceeds supported size".to_owned(),
                ));
            }
            let document: serde_json::Value = serde_json::from_slice(&process.output)
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
            tx.commit()?;
        }
        if changing {
            complete_prepare(
                ctx, &item, &target, &root, &relative, owner, fence, &process,
            )?;
        }
        let fresh = Jobs(ctx)
            .get_item(item_id)?
            .ok_or_else(|| Error::Invalid("job item not found".to_owned()))?;
        let terminal = if shutdown_requested {
            "interrupted"
        } else if process.reason == StopReason::Cancelled {
            "cancelled"
        } else if process.reason == StopReason::Exited && process.exit_code == Some(0) {
            "succeeded"
        } else {
            "failed"
        };
        let version = if fresh.status == "cancel_requested" {
            JobStateMachine(ctx)
                .transition_item(
                    item_id,
                    "cancelling",
                    fresh.status_version,
                    Some(fence),
                    None,
                )?
                .status_version
        } else {
            fresh.status_version
        };
        JobStateMachine(ctx).transition_item(item_id, terminal, version, Some(fence), None)?;
        let code = match process.reason {
            StopReason::TimedOut => Some("timeout"),
            StopReason::Cancelled => Some("cancelled"),
            StopReason::Exited if process.exit_code != Some(0) => Some("verification_failed"),
            StopReason::Exited => None,
        };
        let changed=ctx.conn.execute("UPDATE job_items SET exit_code=?1,error_code=?2 WHERE id=?3 AND executor_fencing_token=?4 AND lease_expires_at>?5 AND EXISTS(SELECT 1 FROM media_executor_leases WHERE id=1 AND fencing_token=?4 AND lease_expires_at>?5)",rusqlite::params![process.exit_code,code,item_id,fence,ctx.clock.now()])?;
        if changed != 1 {
            return Err(Error::Conflict(
                "worker lost its completion authority".to_owned(),
            ));
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
            if authority(ctx, item_id, owner, fence).unwrap_or(false)
                && let Ok(Some(fresh)) = Jobs(ctx).get_item(item_id)
                && fresh.status == "running"
            {
                JobStateMachine(ctx).transition_item(
                    item_id,
                    "failed",
                    fresh.status_version,
                    Some(fence),
                    None,
                )?;
                super::recovery::Recovery(ctx).reconcile_idle_jobs()?;
            }
            gate.release_permit(&permit)?;
            Err(error)
        }
    }
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
            "UPDATE media_files SET compliance_state='unknown',compliance_reasons=?1 WHERE id=?2",
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

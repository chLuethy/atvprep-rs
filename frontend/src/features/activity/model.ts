import type { ChipKind } from "../../ui/StatusChip";
import type { IconName } from "../../ui/icons";
import { N, t } from "../../lib/i18n";
import { baseName } from "../files/model";
import type { ItemRow, JobRow } from "./loaders";

export type Tone = { kind: ChipKind; icon: IconName; label: string; live?: boolean };

const humanize = (value: string) => value.replaceAll("_", " ").replace(/^\w/, (character) => character.toUpperCase());

export function jobTone(status: string): Tone {
  switch (status) {
    case "queued": return { kind: "neutral", icon: "dot", label: t("Queued") };
    case "running": return { kind: "neutral", icon: "activity", label: t("Running"), live: true };
    case "cancel_requested": return { kind: "warn", icon: "warn", label: t("Cancelling") };
    case "succeeded": return { kind: "ok", icon: "check", label: t("Succeeded") };
    case "partial_success": return { kind: "warn", icon: "warn", label: t("Partly failed") };
    case "failed": return { kind: "bad", icon: "x", label: t("Failed") };
    case "cancelled": return { kind: "neutral", icon: "dot", label: t("Cancelled") };
    default: return { kind: "neutral", icon: "dot", label: humanize(status) };
  }
}

export function itemTone(status: string): Tone {
  switch (status) {
    case "stale_input": return { kind: "warn", icon: "warn", label: t("File changed") };
    case "interrupted": return { kind: "warn", icon: "warn", label: t("Interrupted") };
    case "running": return { kind: "neutral", icon: "activity", label: t("Running"), live: true };
    default: return jobTone(status);
  }
}

/** The statuses of a job that still waits or runs; the one definition the list, the filter and the refresh share. */
export const ACTIVE_STATUSES: ReadonlyArray<string> = ["queued", "running", "cancel_requested"];
export const isActive = (job: Pick<JobRow, "status">) => ACTIVE_STATUSES.includes(job.status);
/** The server accepts a cancel only for queued or running jobs. */
export const canCancel = (job: Pick<JobRow, "status">) => job.status === "queued" || job.status === "running";
/** A retry needs failed items and nothing still in flight, which only these two end states guarantee. */
export const canRetry = (job: Pick<JobRow, "status">) => job.status === "failed" || job.status === "partial_success";

const ACTIONS: Record<string, string> = { prepare: N("Prepare"), verify: N("Verify"), inspect: N("Inspect") };
export const actionLabel = (action: string) => (ACTIONS[action] ? t(ACTIONS[action]) : humanize(action));

export function jobTitle(job: JobRow): string {
  const name = job.subject ? baseName(job.subject).title : "";
  if (name) return job.total_count > 1 ? t("{name} and {count} more", { name, count: job.total_count - 1 }) : name;
  return job.total_count > 1 ? t("{action} job for {count} files", { action: actionLabel(job.action), count: job.total_count }) : t("{action} job", { action: actionLabel(job.action) });
}

export function jobProgress(job: JobRow): string {
  const parts = [t("{done} of {total} succeeded", { done: job.succeeded_count, total: job.total_count })];
  if (job.failed_count > 0) parts.push(t("{count} failed", { count: job.failed_count }));
  if (job.cancelled_count > 0) parts.push(t("{count} cancelled", { count: job.cancelled_count }));
  return parts.join(", ");
}

export function formatElapsed(start: string | null, end: string | null): string {
  if (!start || !end) return "";
  const seconds = Math.round((new Date(end).getTime() - new Date(start).getTime()) / 1000);
  if (!Number.isFinite(seconds) || seconds <= 0) return "";
  if (seconds < 60) return `${seconds} s`;
  if (seconds < 3600) {
    const rest = seconds % 60;
    return rest === 0 ? `${Math.floor(seconds / 60)} min` : `${Math.floor(seconds / 60)} min ${rest} s`;
  }
  const minutes = Math.floor((seconds % 3600) / 60);
  return minutes === 0 ? `${Math.floor(seconds / 3600)} h` : `${Math.floor(seconds / 3600)} h ${minutes} min`;
}

/** How far a running job has come, as a whole percent, or undefined when it is not running or nobody knows. */
export function jobPercent(job: Pick<JobRow, "status" | "progress">): number | undefined {
  if (job.status !== "running" || typeof job.progress !== "number" || !Number.isFinite(job.progress)) return undefined;
  return Math.max(0, Math.min(100, Math.round(job.progress)));
}

/** How long something has been running, up to `now`; empty without a start. */
export function runningFor(start: string | null | undefined, now: Date = new Date()): string {
  return start ? formatElapsed(start, now.toISOString()) : "";
}

/** The status of one job item, as the server names it. */
export type ItemStatus =
  | "queued" | "claimed" | "running" | "cancel_requested" | "cancelling"
  | "succeeded" | "failed" | "cancelled" | "interrupted" | "stale_input";

const ENDED_ITEM: ReadonlySet<ItemStatus> = new Set(["succeeded", "failed", "cancelled", "interrupted", "stale_input"]);
const WAITING_ITEM: ReadonlySet<ItemStatus> = new Set(["queued", "claimed"]);
const FAILED_ITEM: ReadonlySet<ItemStatus> = new Set(["failed", "stale_input", "interrupted"]);
/** An item in one of these states will not change again. */
export const isItemTerminal = (status: ItemStatus) => ENDED_ITEM.has(status);
/** An item that ended without doing its work: what „Warum es fehlschlug“ lists. */
export const isItemFailed = (status: ItemStatus) => FAILED_ITEM.has(status);

/** How long an item has taken, as text: running for a while, took a while once it ended, nothing while it waits. */
export function itemDuration(item: Pick<ItemRow, "status" | "started_at" | "finished_at">, now: Date): string {
  if (isItemTerminal(item.status)) {
    const took = formatElapsed(item.started_at ?? null, item.finished_at ?? null);
    return took ? t("took {time}", { time: took }) : "";
  }
  if (WAITING_ITEM.has(item.status)) return "";
  const since = runningFor(item.started_at, now);
  return since ? t("running for {time}", { time: since }) : "";
}

/** Active jobs first; each group keeps the order the server sent (newest first). */
export function partitionJobs(rows: JobRow[]): { active: JobRow[]; history: JobRow[] } {
  return { active: rows.filter(isActive), history: rows.filter((row) => !isActive(row)) };
}

/** Why the worker refused a job for good; it writes these itself, so they are put into the chosen language. */
const REFUSALS: ReadonlyArray<string> = [
  N("The target profile changed after this job was queued."),
  N("The approval is not signed."),
  N("The approval does not belong to this job."),
  N("The approval has expired."),
  N("The job has no recorded file identity."),
  N("This job predates the current policy."),
  N("This job carries no encoding allowance."),
  N("This job carries no target profile."),
  N("The recorded file identity is incomplete."),
  N("The approved file changed."),
  N("The approving session is no longer active."),
];

/** An item's recorded reason, in the chosen language where the worker wrote a known one. */
export function describeDetail(detail: string): string {
  const refused = /^([\s\S]*) Queue the preparation again\.$/.exec(detail);
  const reason = refused?.[1];
  if (reason === undefined || !REFUSALS.includes(reason)) return detail;
  return t("{reason} Queue the preparation again.", { reason: t(reason) });
}

import { N, t, tn } from "../../lib/i18n";
import { formatDateTime } from "../../lib/time";
import type { Action, Schedule } from "./loaders";

export const DAYS: ReadonlyArray<{ index: number; short: string; long: string }> = [
  { index: 0, short: N("Mon"), long: N("Monday") },
  { index: 1, short: N("Tue"), long: N("Tuesday") },
  { index: 2, short: N("Wed"), long: N("Wednesday") },
  { index: 3, short: N("Thu"), long: N("Thursday") },
  { index: 4, short: N("Fri"), long: N("Friday") },
  { index: 5, short: N("Sat"), long: N("Saturday") },
  { index: 6, short: N("Sun"), long: N("Sunday") },
];

export const WEEKDAYS = [0, 1, 2, 3, 4];
export const EVERY_DAY = [0, 1, 2, 3, 4, 5, 6];

const sameDays = (a: number[], b: number[]) => a.length === b.length && a.every((day, index) => day === b[index]);

/** "Every day", "Weekdays", or the days by name. */
export function describeDays(days: number[]): string {
  const sorted = [...days].sort((a, b) => a - b);
  if (sameDays(sorted, EVERY_DAY)) return t("Every day");
  if (sameDays(sorted, WEEKDAYS)) return t("Weekdays");
  if (sameDays(sorted, [5, 6])) return t("Weekends");
  return sorted.map((day) => t(DAYS[day]?.short ?? "")).join(" ");
}

const ACTION_LABEL: Record<Action, string> = { verify: N("Verify"), fix: N("Fix"), convert: N("Convert"), scan: N("Scan") };
export const actionName = (action: Action): string => t(ACTION_LABEL[action]);

const STATE_LABEL: Record<string, string> = {
  unknown: N("Not verified"),
  clean: N("Ready"),
  warning: N("Warning"),
  blocker: N("Blocker"),
  probe_failed: N("Inspection failed"),
};
export const stateName = (state: string): string => (STATE_LABEL[state] ? t(STATE_LABEL[state]) : state);

/** The states a filter can name for an action. */
export function stateChoices(action: Action): ReadonlyArray<{ value: string | null; label: string }> {
  if (action === "scan") return [];
  if (action === "fix" || action === "convert") {
    return [{ value: null, label: t("Any file that needs work") }, { value: "warning", label: t("Warning") }, { value: "blocker", label: t("Blocker") }];
  }
  return [
    { value: "unknown", label: t("Not verified") },
    { value: null, label: t("Any state") },
    { value: "probe_failed", label: t("Inspection failed") },
    { value: "warning", label: t("Warning") },
    { value: "blocker", label: t("Blocker") },
    { value: "clean", label: t("Ready") },
  ];
}

/** The sensible starting filter for an action. */
export const defaultState = (action: Action): string | null => (action === "verify" ? "unknown" : null);

/** What a schedule acts on, in one line. */
export function describeTarget(schedule: Pick<Schedule, "action" | "root_name" | "compliance_state" | "max_files">): string {
  const parts = [actionName(schedule.action), schedule.root_name ?? t("All libraries")];
  if (schedule.compliance_state) parts.push(stateName(schedule.compliance_state));
  if (schedule.max_files !== null) parts.push(tn("up to {count} file", "up to {count} files", schedule.max_files));
  return parts.join(" · ");
}

export function describeNext(schedule: Pick<Schedule, "enabled" | "next_run_at">): string {
  if (!schedule.enabled) return t("Paused");
  return schedule.next_run_at ? t("Next {when}", { when: formatDateTime(schedule.next_run_at, t("unknown")) }) : t("No upcoming run");
}

export function describeLast(schedule: Pick<Schedule, "last_run_at" | "last_result">): string {
  if (!schedule.last_run_at) return t("Never run");
  return `${describeResult(schedule.last_result)} · ${formatDateTime(schedule.last_run_at, "")}`.trim();
}

const NOT_APPROVED = "not approved to change files. Turn the schedule on again.";
N("Not approved to change files. Turn the schedule on again.");

/** What the last run says. The server writes it in English; the ones it writes itself are put into the chosen language. */
export function describeResult(result: string | null): string {
  if (!result) return t("Ran");
  const queued = /^Queued (\d+) files?$/.exec(result);
  if (queued) return tn("Queued {count} file", "Queued {count} files", Number(queued[1]));
  const scans = /^Queued (\d+) scans?$/.exec(result);
  if (scans) return tn("Queued {count} scan", "Queued {count} scans", Number(scans[1]));
  if (result === "No libraries") return t("No libraries");
  if (result === "No matching files") return t("No matching files");
  const refused = /^Not queued: ([\s\S]*)$/.exec(result);
  if (refused) {
    const reason = refused[1] ?? "";
    return t("Not queued: {reason}", { reason: reason === NOT_APPROVED ? t("Not approved to change files. Turn the schedule on again.") : reason });
  }
  return result;
}

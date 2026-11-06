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

/** The cron the server stores for days (Monday is 0) and a time, written the way the server writes it. */
export function weeklyCron(days: number[], time: string): string {
  const [hour = "0", minute = "0"] = time.split(":");
  const weekdays = [...new Set(days.map((day) => (day + 1) % 7))].sort((a, b) => a - b);
  return `${Number(minute)} ${Number(hour)} * * ${weekdays.join(",")}`;
}

/**
 * The days and time a cron stands for, when the simple mode can say the same: one minute, one hour,
 * any day of the month and month, and weekdays as numbers or plain ranges. Anything else is null.
 * This only decides which editor mode fits; the meaning of an expression comes from the server.
 */
export function simpleForm(cron: string): { days: number[]; time: string } | null {
  const match = /^(\d{1,2}) (\d{1,2}) \* \* (\*|\d(?:-\d)?(?:,\d(?:-\d)?)*)$/.exec(cron.trim());
  if (!match) return null;
  const minute = Number(match[1]);
  const hour = Number(match[2]);
  if (minute > 59 || hour > 23) return null;
  const days = new Set<number>();
  for (const part of (match[3] ?? "*") === "*" ? ["0-6"] : (match[3] ?? "").split(",")) {
    const [from = 0, to = from] = part.split("-").map(Number);
    if (from > to || to > 7) return null;
    for (let weekday = from; weekday <= to; weekday += 1) days.add((weekday + 6) % 7);
  }
  const pad = (value: number) => String(value).padStart(2, "0");
  return { days: [...days].sort((a, b) => a - b), time: `${pad(hour)}:${pad(minute)}` };
}

/** When a schedule runs: days and time if the simple mode can say it, else the cron itself. */
export function describeWhen(schedule: Pick<Schedule, "cron" | "days" | "time">): string {
  return simpleForm(schedule.cron) ? `${describeDays(schedule.days)} · ${schedule.time}` : schedule.cron;
}

/** Cron examples the guide offers, each with what it means. */
export const CRON_EXAMPLES: ReadonlyArray<{ cron: string; meaning: string }> = [
  { cron: "0 3 * * 1-5", meaning: N("Weekdays at 03:00") },
  { cron: "*/15 * * * *", meaning: N("Every 15 minutes") },
  { cron: "0 */6 * * *", meaning: N("Every 6 hours, on the hour") },
];

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

export type SortKey = "next" | "name" | "last";
export type StatusFilter = "on" | "paused";
export type ListFilter = { actions: Action[]; statuses: StatusFilter[] };

export const SORT_KEYS: ReadonlyArray<{ key: SortKey; label: string }> = [
  { key: "next", label: N("Next run") },
  { key: "name", label: N("Name") },
  { key: "last", label: N("Last run") },
];
export const LIST_ACTIONS: ReadonlyArray<Action> = ["verify", "fix", "convert", "scan"];
export const LIST_STATUSES: ReadonlyArray<{ status: StatusFilter; label: string }> = [
  { status: "on", label: N("On") },
  { status: "paused", label: N("Paused") },
];

const time = (value: string | null): number | null => {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : parsed;
};

/** A sorted copy: next run soonest first, last run newest first; schedules without that time come last. */
export function sortSchedules(rows: readonly Schedule[], key: SortKey): Schedule[] {
  const byName = (a: Schedule, b: Schedule) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  const byTime = (pick: (row: Schedule) => string | null, newestFirst: boolean) => (a: Schedule, b: Schedule) => {
    const left = time(pick(a));
    const right = time(pick(b));
    if (left === null && right === null) return byName(a, b);
    if (left === null) return 1;
    if (right === null) return -1;
    return (newestFirst ? right - left : left - right) || byName(a, b);
  };
  const compare = key === "name" ? byName : key === "last" ? byTime((row) => row.last_run_at, true) : byTime((row) => row.next_run_at, false);
  return [...rows].sort(compare);
}

/** Rows matching any chosen action and any chosen status; an empty choice matches everything. */
export function filterSchedules(rows: readonly Schedule[], filter: ListFilter): Schedule[] {
  return rows.filter((row) =>
    (filter.actions.length === 0 || filter.actions.includes(row.action)) &&
    (filter.statuses.length === 0 || filter.statuses.includes(row.enabled ? "on" : "paused")));
}

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
  if (refused) return t("Not queued: {reason}", { reason: refused[1] ?? "" });
  return result;
}

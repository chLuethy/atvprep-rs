import { N } from "../../lib/i18n";
import type { FacetCounts } from "../../ui/FilterBar";

/**
 * The activity filter and its address. Status and action: nothing chosen shows every job, chosen values show
 * the jobs with any of them (OR); the server takes them as `status` and `action`. The search is `q`, the order
 * `sort` (newest first unless `sort=oldest`). The Auslöser is `trigger` with single schedules as `schedule`,
 * alternatives to each other (OR) within one facet. New facets and narrowings (#71 period
 * and library, #72 search over all items) go into `JobFilter`, `parseJobFilter`, `jobFilterQuery` and
 * `jobListParams` alike.
 */
export type JobFilterEntry = { token: string; label: string; urgent?: boolean };

/** Every job status, in the order of a job's life. */
export const JOB_STATUSES: ReadonlyArray<JobFilterEntry> = [
  { token: "queued", label: N("Queued") },
  { token: "running", label: N("Running") },
  { token: "cancel_requested", label: N("Cancelling") },
  { token: "succeeded", label: N("Succeeded") },
  { token: "partial_success", label: N("Partly failed"), urgent: true },
  { token: "failed", label: N("Failed"), urgent: true },
  { token: "cancelled", label: N("Cancelled") },
];

/** The actions as the API names them. */
export const JOB_ACTIONS: ReadonlyArray<JobFilterEntry> = [
  { token: "verify", label: N("Verify") },
  { token: "prepare", label: N("Prepare") },
  { token: "inspect", label: N("Inspect") },
];

/** The Auslöser as the server derives them, in popover order; `schedule` is the group of every schedule. */
export const JOB_TRIGGERS: ReadonlyArray<JobFilterEntry> = [
  { token: "manual", label: N("By hand") },
  { token: "retry", label: N("Retried") },
  { token: "schedule", label: N("Schedules") },
];

/** The statuses of a job that still waits or runs. */
export const ACTIVE_STATUSES: ReadonlyArray<string> = ["queued", "running", "cancel_requested"];

export type JobSort = "newest" | "oldest";

export type JobFilter = {
  /** The search, trimmed; "" for none. */
  q: string;
  /** Chosen statuses in canonical order; none means every status. */
  status: string[];
  /** Chosen actions in canonical order; none means every action. */
  action: string[];
  /** Chosen Auslöser in canonical order; missing or none means every one. */
  trigger?: string[];
  /** Chosen single schedules (opaque ids, sorted), alternatives to `trigger`; missing or none means none. */
  schedule?: string[];
  sort: JobSort;
};

export const EMPTY_JOB_FILTER: JobFilter = { q: "", status: [], action: [], sort: "newest" };

const STATUS_ORDER = JOB_STATUSES.map((entry) => entry.token);
const ACTION_ORDER = JOB_ACTIONS.map((entry) => entry.token);
const TRIGGER_ORDER = JOB_TRIGGERS.map((entry) => entry.token);
/** What an opaque schedule id looks like; anything else would be refused by the server. */
const SCHEDULE_ID = /^sc_[A-Za-z0-9_-]+$/;
const scheduleIds = (values: Iterable<string>): string[] => [...new Set(values)].filter((id) => SCHEDULE_ID.test(id)).sort();

/** The Auslöser part of a filter, only with something chosen, so a filter without it stays as it was. */
function triggerPart(trigger: Iterable<string>, schedule: Iterable<string>): Pick<JobFilter, "trigger" | "schedule"> {
  const part: Pick<JobFilter, "trigger" | "schedule"> = {};
  const triggers = canonical(TRIGGER_ORDER, trigger);
  const schedules = scheduleIds(schedule);
  if (triggers.length > 0) part.trigger = triggers;
  if (schedules.length > 0) part.schedule = schedules;
  return part;
}

const list = (value: string | null): string[] => (value ?? "").split(",").filter(Boolean);
/** Known values only, once each, in the order of `order`. */
const canonical = (order: ReadonlyArray<string>, values: Iterable<string>): string[] => {
  const wanted = new Set(values);
  return order.filter((token) => wanted.has(token));
};

/** The filter an address asks for; unknown values are dropped. */
export function parseJobFilter(params: URLSearchParams): JobFilter {
  return {
    q: (params.get("q") ?? "").trim(),
    status: canonical(STATUS_ORDER, list(params.get("status"))),
    action: canonical(ACTION_ORDER, list(params.get("action"))),
    ...triggerPart(list(params.get("trigger")), list(params.get("schedule"))),
    sort: params.get("sort") === "oldest" ? "oldest" : "newest",
  };
}

/** The lists and the search, as both the address and the server take them. */
function filterParts(filter: JobFilter): string[] {
  const parts: string[] = [];
  const status = canonical(STATUS_ORDER, filter.status);
  const action = canonical(ACTION_ORDER, filter.action);
  if (status.length > 0) parts.push(`status=${status.join(",")}`);
  if (action.length > 0) parts.push(`action=${action.join(",")}`);
  const { trigger, schedule } = triggerPart(filter.trigger ?? [], filter.schedule ?? []);
  if (trigger) parts.push(`trigger=${trigger.join(",")}`);
  if (schedule) parts.push(`schedule=${schedule.join(",")}`);
  if (filter.q.trim()) parts.push(`q=${encodeURIComponent(filter.q.trim())}`);
  return parts;
}

/** The canonical query string of a filter, without "?": `status`, `action`, `q`, then `sort`; "" for none. */
export function jobFilterQuery(filter: JobFilter): string {
  const parts = filterParts(filter);
  if (filter.sort === "oldest") parts.push("sort=oldest");
  return parts.join("&");
}

/** The query of `/jobs` and `/jobs/counts` for a filter, without paging. */
export function jobListParams(filter: JobFilter): string {
  return [`order=${filter.sort === "oldest" ? "asc" : "desc"}`, ...filterParts(filter)].join("&");
}

/** Anything at all narrows the list; the order does not. */
export const isJobFiltered = (filter: JobFilter): boolean =>
  filter.q !== "" || filter.status.length > 0 || filter.action.length > 0 || (filter.trigger?.length ?? 0) > 0 || (filter.schedule?.length ?? 0) > 0;

/** Two filters show the same jobs in the same order. */
export const sameJobFilter = (a: JobFilter, b: JobFilter): boolean => jobFilterQuery(a) === jobFilterQuery(b);

/** The facet counters of the job list, plus how many waiting or running jobs the filter leaves out. */
export type JobCounts = {
  facets: FacetCounts;
  matched: number;
  total: number;
  hiddenActive: number;
  /** The schedules a job can come from, by name, for the facet Auslöser. */
  schedules?: ReadonlyArray<{ id: string; name: string }>;
};

/**
 * A filter that shows the active jobs the current one hides. When only the status choice hides them (the
 * status counts already apply every other filter), the active statuses are added; otherwise every filter is
 * cleared, the order kept.
 */
export function revealActive(filter: JobFilter, counts: JobCounts): JobFilter {
  const status = counts.facets.status ?? {};
  const passOthers = ACTIVE_STATUSES.reduce((sum, token) => sum + (status[token] ?? 0), 0);
  const shown = filter.status.length === 0 ? passOthers : ACTIVE_STATUSES.filter((token) => filter.status.includes(token)).reduce((sum, token) => sum + (status[token] ?? 0), 0);
  if (filter.status.length > 0 && passOthers === shown + counts.hiddenActive) {
    return { ...filter, status: canonical(STATUS_ORDER, [...filter.status, ...ACTIVE_STATUSES]) };
  }
  return { ...EMPTY_JOB_FILTER, sort: filter.sort };
}

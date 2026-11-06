import { SessionError } from "../../lib/api";
import { rawId } from "../files/loaders";

export type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

/** The four things a schedule can do. Fix and Convert change files. */
export type Action = "verify" | "fix" | "convert" | "scan";

/** Whether a schedule of this action changes files: it starts paused and is turned on only after a confirmation. */
export const changesFiles = (action: Action): boolean => action === "fix" || action === "convert";

export type Schedule = {
  id: string;
  name: string;
  action: Action;
  root_id: string | null;
  root_name: string | null;
  compliance_state: string | null;
  /** The 5-field cron the schedule runs on (server time, Sunday is 0). The simple mode derives it from days and time. */
  cron: string;
  /** Weekdays the schedule runs on, Monday is 0. */
  days: number[];
  /** HH:MM in the server's time zone. */
  time: string;
  /** Null for a scan: it has no file limit. */
  max_files: number | null;
  enabled: boolean;
  /** How many files it would pick right now. */
  matching: number | null;
  next_run_at: string | null;
  last_run_at: string | null;
  last_result: string | null;
  last_job_id: string | null;
};

export type SchedulesResult = { kind: "ok"; items: Schedule[] } | { kind: "error" } | { kind: "session" };

export async function loadSchedules(api: Api): Promise<SchedulesResult> {
  try {
    const response = await api.request("/schedules");
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: Schedule[] };
    return { kind: "ok", items: body.data ?? [] };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

/** The fields a person edits. `rootId` and `complianceState` of null mean "any". */
export type ScheduleInput = {
  name: string;
  action: Action;
  rootId: string | null;
  complianceState: string | null;
  days: number[];
  time: string;
  /** A cron expression (the Cron mode); null means the days and time above (the Simple mode). */
  cron: string | null;
  maxFiles: number;
};

export type SaveResult = { ok: true; schedule: Schedule } | { ok: false; session: boolean; detail?: string };
export type DoneResult = { ok: true; schedule?: Schedule } | { ok: false; session: boolean; detail?: string };

async function failure(response: Response): Promise<{ ok: false; session: boolean; detail?: string }> {
  const reason = (await response.json().catch(() => null)) as { detail?: unknown } | null;
  // Only a plain sentence is worth showing; any structured detail is ignored.
  return typeof reason?.detail === "string" ? { ok: false, session: false, detail: reason.detail } : { ok: false, session: false };
}

async function send(api: Api, path: string, method: string, key: string, body?: unknown): Promise<DoneResult> {
  try {
    const response = await api.request(path, {
      method,
      headers: { "Idempotency-Key": key, ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    if (!response.ok) return await failure(response);
    const parsed = (await response.json().catch(() => ({}))) as { data?: Schedule };
    return parsed.data && "id" in parsed.data ? { ok: true, schedule: parsed.data } : { ok: true };
  } catch (error) {
    return { ok: false, session: error instanceof SessionError };
  }
}

const timing = (input: Pick<ScheduleInput, "cron" | "days" | "time">) => (input.cron === null ? { days: input.days, time: input.time } : { cron: input.cron });

const idPath = (id: string) => `/schedules/${encodeURIComponent(rawId(id))}`;

export async function createSchedule(api: Api, input: ScheduleInput, key: string): Promise<SaveResult> {
  const result = await send(api, "/schedules", "POST", key, {
    name: input.name, action: input.action, root_id: input.rootId,
    ...(input.action === "scan" ? {} : { compliance_state: input.complianceState }),
    ...timing(input),
    ...(input.action === "scan" ? {} : { max_files: input.maxFiles }),
  });
  return result.ok && result.schedule ? { ok: true, schedule: result.schedule } : result.ok ? { ok: false, session: false } : result;
}

export async function updateSchedule(api: Api, id: string, input: ScheduleInput, key: string): Promise<DoneResult> {
  const scan = input.action === "scan";
  return send(api, idPath(id), "PATCH", key, {
    name: input.name,
    root_id: input.rootId, clear_root: input.rootId === null,
    ...(scan ? {} : { compliance_state: input.complianceState, clear_compliance_state: input.complianceState === null, max_files: input.maxFiles }),
    ...timing(input),
  });
}

/** `confirmed` is the person's explicit yes to a schedule that changes files; the server signs the schedule on it. */
export const setEnabled = (api: Api, id: string, enabled: boolean, key: string, confirmed = false) =>
  send(api, idPath(id), "PATCH", key, { enabled, ...(enabled && confirmed ? { confirm_changes_files: true } : {}) });
export const deleteSchedule = (api: Api, id: string, key: string) => send(api, idPath(id), "DELETE", key);
export const runScheduleNow = (api: Api, id: string, key: string) => send(api, `${idPath(id)}/run`, "POST", key);

/** What the server says about a cron expression someone is typing. */
export type CronPreview =
  | { kind: "ok"; description: string; nextRuns: string[] }
  | { kind: "invalid"; error: string }
  | { kind: "error" };

export async function previewCron(api: Api, expression: string, lang: string): Promise<CronPreview> {
  try {
    const response = await api.request(`/schedules/cron-preview?expression=${encodeURIComponent(expression)}&lang=${encodeURIComponent(lang)}`);
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: { valid?: boolean; description?: string; next_runs?: string[]; error?: string } };
    const data = body.data;
    if (data?.valid === true) return { kind: "ok", description: data.description ?? "", nextRuns: data.next_runs ?? [] };
    if (data?.valid === false) return { kind: "invalid", error: data.error ?? "" };
    return { kind: "error" };
  } catch {
    return { kind: "error" };
  }
}

import { SessionError } from "../../lib/api";
import { rawId } from "../files/loaders";

export type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

export type Action = "verify" | "prepare";

export type Schedule = {
  id: string;
  name: string;
  action: Action;
  root_id: string | null;
  root_name: string | null;
  compliance_state: string | null;
  /** Weekdays the schedule runs on, Monday is 0. */
  days: number[];
  /** HH:MM in the server's time zone. */
  time: string;
  max_files: number;
  enabled: boolean;
  /** How many files it would pick right now. */
  matching: number;
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
  maxFiles: number;
};

export type SaveResult = { ok: true; schedule: Schedule } | { ok: false; session: boolean; detail?: string };
export type DoneResult = { ok: true; schedule?: Schedule } | { ok: false; session: boolean; detail?: string };

async function failure(response: Response): Promise<{ ok: false; session: boolean; detail?: string }> {
  const reason = (await response.json().catch(() => null)) as { detail?: unknown } | null;
  // FastAPI sends a list of problems for a malformed body; only a plain sentence is worth showing.
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

const idPath = (id: string) => `/schedules/${encodeURIComponent(rawId(id))}`;

export async function createSchedule(api: Api, input: ScheduleInput, key: string): Promise<SaveResult> {
  const result = await send(api, "/schedules", "POST", key, {
    name: input.name, action: input.action, root_id: input.rootId, compliance_state: input.complianceState,
    days: input.days, time: input.time, max_files: input.maxFiles,
  });
  return result.ok && result.schedule ? { ok: true, schedule: result.schedule } : result.ok ? { ok: false, session: false } : result;
}

export async function updateSchedule(api: Api, id: string, input: Omit<ScheduleInput, "action">, key: string): Promise<DoneResult> {
  return send(api, idPath(id), "PATCH", key, {
    name: input.name,
    root_id: input.rootId, clear_root: input.rootId === null,
    compliance_state: input.complianceState, clear_compliance_state: input.complianceState === null,
    days: input.days, time: input.time, max_files: input.maxFiles,
  });
}

export const setEnabled = (api: Api, id: string, enabled: boolean, key: string) => send(api, idPath(id), "PATCH", key, { enabled });
export const deleteSchedule = (api: Api, id: string, key: string) => send(api, idPath(id), "DELETE", key);
export const runScheduleNow = (api: Api, id: string, key: string) => send(api, `${idPath(id)}/run`, "POST", key);

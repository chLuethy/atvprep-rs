import { SessionError } from "../../lib/api";
import { rawId } from "../files/loaders";

export type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

/** The three things a schedule can do. Prepare changes files. */
export type Action = "verify" | "prepare" | "scan";

/** Whether a schedule of this action changes files: it starts paused and is turned on only after a confirmation. */
export const changesFiles = (action: Action): boolean => action === "prepare";

/** The Kodierfreigabe of a schedule that prepares, in rising order (ADR 0008). */
export type EncodeLevel = "copy" | "audio" | "all";
export const ENCODE_LEVELS: ReadonlyArray<EncodeLevel> = ["copy", "audio", "all"];

/** The Zielprofil as the server reports it: what a schedule is signed against when it is turned on. */
export type TargetProfile = { crf: number; prefer_hevc10: boolean; keep_av1: boolean; fix_dts: boolean };

/** One entry of a schedule's history: a run at its slot or by hand. */
export type ScheduleRun = {
  /** A run that was started, or a slot the manager did not trigger in time. */
  kind?: "run" | "missed";
  started_at: string;
  trigger: "scheduled" | "manual";
  result: string;
  job_id: string | null;
  scan_id: string | null;
};

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
  /** Prepare only: the Kodierfreigabe. Part of what the person signs when turning it on. */
  encode?: EncodeLevel | null;
  /** Prepare only: the current Zielprofil, which turning the schedule on signs as well. */
  profile?: TargetProfile | null;
  enabled: boolean;
  /** How many files it would pick right now. */
  matching: number | null;
  next_run_at: string | null;
  last_run_at: string | null;
  last_result: string | null;
  last_job_id: string | null;
  /** The latest runs, newest first. */
  runs?: ScheduleRun[];
};

/** The limits the server enforces on a schedule; the page shows them but never decides them. */
export type Limits = {
  /** The shortest gap between two runs of a schedule that changes files. */
  minChangeIntervalMinutes: number;
  /** The most files per run, by action. A scan has none. */
  maxFiles: Partial<Record<Action, number>>;
  /** The most files per run of a schedule that prepares with the video in its Kodierfreigabe. */
  maxFilesVideo: number | null;
};

/** The most files one run may take: the video level of preparing has its own, lower limit. Null when the server has not said. */
export function maxFilesLimit(limits: Limits | null, action: Action, encode: EncodeLevel): number | null {
  if (!limits) return null;
  if (action === "prepare" && encode === "all") return limits.maxFilesVideo ?? limits.maxFiles.prepare ?? null;
  return limits.maxFiles[action] ?? null;
}

export type SchedulesResult = { kind: "ok"; items: Schedule[]; limits: Limits | null } | { kind: "error" } | { kind: "session" };

type LimitsWire = { min_change_interval_minutes?: unknown; max_files?: Record<string, unknown>; max_files_video?: unknown };

function readLimits(wire: LimitsWire | undefined): Limits | null {
  if (!wire || typeof wire.min_change_interval_minutes !== "number") return null;
  const maxFiles: Limits["maxFiles"] = {};
  for (const action of ["verify", "prepare", "scan"] as const) {
    const value = wire.max_files?.[action];
    if (typeof value === "number") maxFiles[action] = value;
  }
  const maxFilesVideo = typeof wire.max_files_video === "number" ? wire.max_files_video : null;
  return { minChangeIntervalMinutes: wire.min_change_interval_minutes, maxFiles, maxFilesVideo };
}

export async function loadSchedules(api: Api): Promise<SchedulesResult> {
  try {
    const response = await api.request("/schedules");
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: Schedule[]; meta?: { limits?: LimitsWire } };
    return { kind: "ok", items: body.data ?? [], limits: readLimits(body.meta?.limits) };
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
  /** Prepare only. A new schedule only copies. */
  encode: EncodeLevel;
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
    ...(changesFiles(input.action) ? { encode: input.encode } : {}),
  });
  return result.ok && result.schedule ? { ok: true, schedule: result.schedule } : result.ok ? { ok: false, session: false } : result;
}

export async function updateSchedule(api: Api, id: string, input: ScheduleInput, key: string): Promise<DoneResult> {
  const scan = input.action === "scan";
  return send(api, idPath(id), "PATCH", key, {
    name: input.name,
    root_id: input.rootId, clear_root: input.rootId === null,
    ...(scan ? {} : { compliance_state: input.complianceState, clear_compliance_state: input.complianceState === null, max_files: input.maxFiles }),
    ...(changesFiles(input.action) ? { encode: input.encode } : {}),
    ...timing(input),
  });
}

/** `confirmed` is the person's explicit yes to a schedule that changes files; the server signs the schedule on it. */
export const setEnabled = (api: Api, id: string, enabled: boolean, key: string, confirmed = false) =>
  send(api, idPath(id), "PATCH", key, { enabled, ...(enabled && confirmed ? { confirm_changes_files: true } : {}) });
export const deleteSchedule = (api: Api, id: string, key: string) => send(api, idPath(id), "DELETE", key);
export const runScheduleNow = (api: Api, id: string, key: string) => send(api, `${idPath(id)}/run`, "POST", key);

/** What the server says about a cron expression someone is typing. */
/** What the person is told before turning on a schedule that changes files, read fresh from the server. */
export type EnableFacts = { matching: number; deleteOriginal: boolean | null; encode: EncodeLevel | null; profile: TargetProfile | null };

/** The schedule's current match count (capped by its run size) and the `delete_original` setting; null when the schedule cannot be read. */
export async function loadEnableFacts(api: Api, id: string): Promise<EnableFacts | { session: true } | null> {
  try {
    const listed = await loadSchedules(api);
    if (listed.kind === "session") return { session: true };
    const found = listed.kind === "ok" ? listed.items.find((item) => item.id === id) : undefined;
    if (!found) return null;
    let deleteOriginal: boolean | null = null;
    try {
      const response = await api.request("/dashboard");
      if (response.ok) deleteOriginal = ((await response.json()) as { data?: { delete_original?: boolean } }).data?.delete_original ?? null;
    } catch (error) {
      if (error instanceof SessionError) return { session: true };
    }
    const matching = found.matching ?? 0;
    return {
      matching: found.max_files === null ? matching : Math.min(matching, found.max_files),
      deleteOriginal,
      encode: found.encode ?? null,
      profile: found.profile ?? null,
    };

  } catch {
    return null;
  }
}

export type CronPreview =
  | { kind: "ok"; description: string; nextRuns: string[] }
  | { kind: "invalid"; error: string }
  | { kind: "error" };

/** `action` lets the server apply that action's limits (the minimum interval) to the expression. */
export async function previewCron(api: Api, expression: string, lang: string, action: Action): Promise<CronPreview> {
  try {
    const response = await api.request(`/schedules/cron-preview?expression=${encodeURIComponent(expression)}&lang=${encodeURIComponent(lang)}&action=${action}`);
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

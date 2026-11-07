import { SessionError } from "../../lib/api";

/** Data loading for the library and the inspector. UI-free; every loader normalises SessionError. */

export type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

export type FileRow = {
  id: string;
  root_id: string;
  display_path?: string;
  kind: string;
  suffix: string;
  size: number;
  mtime_ns: number;
  discovery_state: string;
  compliance_state: string;
  compliance_reasons?: string[];
  workflow_state: string;
  desired_action: string;
  last_seen_at: string | null;
  /** "queued", "running" or "cancelling" while a fix for this file is still in progress. */
  fix_status?: string | null;
  /** External .srt files a fix would still merge into the file. */
  pending_sidecars?: number;
  /** The verify report says the video has to be re-encoded, which Prepare only does after a confirmation. */
  needs_reencode?: boolean;
  /** Another container whose converted MP4 is already kept next to it; there is nothing left to prepare. */
  converted_kept?: boolean;
  /** The same files by name, language (three letter code) and kind (normal, forced, sdh). */
  embed_sidecars?: Array<{ name: string; language: string; kind: string }>;
};

export type FileDetail = FileRow & {
  status_version: number;
  last_success_at: string | null;
  has_error: boolean;
  sidecar_count: number;
  inspection_available: boolean;
};

export type Job = {
  id: string;
  action: string;
  status: string;
  trigger: string;
  origin_job_id: string | null;
  requested_at: string | null;
  started_at: string | null;
  finished_at: string | null;
  total_count: number;
  succeeded_count: number;
  failed_count: number;
  cancelled_count: number;
};

export type Inspection = {
  id: string;
  media_id: string;
  tool_name: string;
  tool_version: string;
  result: Record<string, unknown>;
  created_at: string | null;
};

export type Sidecar = {
  id: string; media_id: string; name: string; size: number; mtime_ns: number; fingerprint: string; created_at: string | null;
  /** What a fix does with it: pending, embedded, ignored_language, unknown_language, unsupported or unchecked. */
  state?: string;
  /** Three letter language code read from the name, "und" when none. */
  language?: string;
};

export type LoadState<T> = { kind: "ok" | "error" | "session"; items: T[] };
export type DetailState = { kind: "ok"; data: FileDetail } | { kind: "error" } | { kind: "session" };
export type InspectionState = { items: Inspection[]; error: boolean };
export type MountedInspectionState = InspectionState & { kind: "ok" | "session" };
export type SidecarState = { kind: "ok"; items: Sidecar[]; nextCursor: string | null } | { kind: "error" } | { kind: "session" };

export async function loadDetail(api: Api, id: string): Promise<FileDetail | null> {
  try {
    const response = await api.request(`/files/${id}`);
    if (!response.ok) return null;
    const body = (await response.json()) as { data?: FileDetail };
    return body.data ?? null;
  } catch (error) {
    if (error instanceof SessionError) throw error;
    return null;
  }
}

export async function loadDetailMounted(api: Api, id: string): Promise<DetailState> {
  try {
    const data = await loadDetail(api, id);
    return data ? { kind: "ok", data } : { kind: "error" };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

export async function loadJobs(api: Api, id: string): Promise<{ items: Job[]; error: boolean }> {
  try {
    const response = await api.request(`/files/${id}/jobs?limit=50`);
    if (!response.ok) return { items: [], error: true };
    const body = (await response.json()) as { data?: Job[] };
    return { items: body.data ?? [], error: false };
  } catch (error) {
    if (error instanceof SessionError) throw error;
    return { items: [], error: true };
  }
}

export async function loadJobsMounted(api: Api, id: string): Promise<LoadState<Job>> {
  try {
    const state = await loadJobs(api, id);
    return { kind: state.error ? "error" : "ok", items: state.items };
  } catch (error) {
    return { kind: error instanceof SessionError ? "session" : "error", items: [] };
  }
}

export async function loadInspections(api: Api, mediaId: string): Promise<InspectionState> {
  try {
    const response = await api.request(`/files/${mediaId}/inspections?limit=50`);
    if (!response.ok) return { items: [], error: true };
    const body = (await response.json()) as { data?: Inspection[] };
    return { items: body.data ?? [], error: false };
  } catch (error) {
    if (error instanceof SessionError) throw error;
    return { items: [], error: true };
  }
}

export async function loadInspectionsMounted(api: Api, mediaId: string): Promise<MountedInspectionState> {
  try {
    return { ...(await loadInspections(api, mediaId)), kind: "ok" };
  } catch (error) {
    if (error instanceof SessionError) return { items: [], error: true, kind: "session" };
    return { items: [], error: true, kind: "ok" };
  }
}

export async function loadSidecarsPage(api: Api, mediaId: string, cursor?: string): Promise<SidecarState> {
  try {
    const response = await api.request(`/files/${mediaId}/sidecars?limit=50${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`);
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: Sidecar[]; meta?: { next_cursor?: string | null } };
    return { kind: "ok", items: body.data ?? [], nextCursor: body.meta?.next_cursor ?? null };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

export async function loadSidecars(api: Api, mediaId: string): Promise<SidecarState> {
  return loadSidecarsPage(api, mediaId);
}

export type FilesQuery = { q: string; complianceState: string; /** Comma separated states whose files are left out. */ hide?: string; cursor: string | null; sort?: { key: string; dir: 1 | -1 } | null };
export type FilePage = { items: FileRow[]; nextCursor: string | null; error: boolean; session: boolean };

export async function loadFiles(api: Api, query: FilesQuery): Promise<FilePage> {
  const params = new URLSearchParams({ limit: "50" });
  if (query.q) params.set("q", query.q);
  if (query.complianceState) params.set("compliance_state", query.complianceState);
  if (query.hide) params.set("hide", query.hide);
  if (query.cursor) params.set("cursor", query.cursor);
  if (query.sort) {
    params.set("sort", query.sort.key);
    params.set("dir", query.sort.dir === 1 ? "asc" : "desc");
  }
  try {
    const response = await api.request(`/files?${params.toString()}`);
    if (!response.ok) return { items: [], nextCursor: null, error: true, session: false };
    const body = (await response.json()) as { data: FileRow[]; meta?: { next_cursor?: string | null } };
    return { items: Array.isArray(body.data) ? body.data : [], nextCursor: body.meta?.next_cursor ?? null, error: false, session: false };
  } catch (error) {
    return { items: [], nextCursor: null, error: true, session: error instanceof SessionError };
  }
}

export type Summary = {
  counts: Record<string, number> | null;
  missing: number;
  convert: number;
  /** Files whose video has to be re-encoded. */
  reencode: number;
  /** Other containers whose converted MP4 is kept next to them. */
  converted: number;
  jobsActive: boolean;
  jobs: { running: number; queued: number };
};

/** File counts per compliance state for the filter chips, and whether any job is still waiting or running. */
export async function loadSummary(api: Api): Promise<Summary | null> {
  try {
    const response = await api.request("/dashboard");
    if (!response.ok) return null;
    const body = (await response.json()) as { data?: { files?: Record<string, number>; missing?: number; convert?: number; reencode?: number; converted?: number; jobs?: Record<string, number> } };
    const jobs = body.data?.jobs ?? {};
    return { counts: body.data?.files ?? null, missing: body.data?.missing ?? 0, convert: body.data?.convert ?? 0, reencode: body.data?.reencode ?? 0, converted: body.data?.converted ?? 0, jobsActive: (jobs.queued ?? 0) + (jobs.running ?? 0) > 0, jobs: { running: jobs.running ?? 0, queued: jobs.queued ?? 0 } };
  } catch {
    return null;
  }
}

/** File counts per compliance state, for the filter chips. A failure just hides the counts. */
export async function loadCounts(api: Api): Promise<Record<string, number> | null> {
  return (await loadSummary(api))?.counts ?? null;
}

export type DismissResult = { ok: true; removed: number } | { ok: false; session: boolean };

/** Hide every missing file from the library. The caller owns the idempotency key so a retried attempt reuses it. */
export async function dismissMissing(api: Api, idempotencyKey: string): Promise<DismissResult> {
  try {
    const response = await api.request("/files/dismiss-missing", { method: "POST", headers: { "Idempotency-Key": idempotencyKey } });
    if (!response.ok) return { ok: false, session: false };
    const body = (await response.json().catch(() => ({}))) as { data?: { removed?: number } };
    return { ok: true, removed: body.data?.removed ?? 0 };
  } catch (error) {
    return { ok: false, session: error instanceof SessionError };
  }
}

/** `detail` is the reason the server gave, when it gave one. */
/** An id taken from the address may still be percent-encoded (the `=` of a token is `%3D`); the server wants it raw. */
export function rawId(id: string): string {
  try { return decodeURIComponent(id); } catch { return id; }
}

export type JobResult = { ok: true } | { ok: false; session: boolean; detail?: string };

/**
 * Queues a manual job: verify reads MP4 files, inspect reads any file again with ffprobe, prepare makes any
 * container an MP4 that Apple TV plays. The server takes at most 100 files per job.
 * Starting a prepare is the explicit approval (`manual_approval`), bound to the session by the server.
 * It never asks to delete an original. A prepare that includes a file needing a re-encode only runs with
 * `confirmReencode`; the server refuses it otherwise. The caller owns the idempotency key so a retried
 * attempt reuses it.
 */
export async function queueJob(
  api: Api,
  action: "prepare" | "verify" | "inspect",
  mediaIds: string[],
  idempotencyKey: string,
  options: { confirmReencode?: boolean } = {},
): Promise<JobResult> {
  const body = action === "prepare"
    ? { action, media_ids: mediaIds, manual_approval: true, ...(options.confirmReencode ? { confirm_reencode: true } : {}) }
    : { action, media_ids: mediaIds, manual_approval: false };
  try {
    const response = await api.request("/jobs", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey },
      body: JSON.stringify(body),
    });
    if (response.ok) return { ok: true };
    const reason = (await response.json().catch(() => null)) as { detail?: unknown } | null;
    return typeof reason?.detail === "string" ? { ok: false, session: false, detail: reason.detail } : { ok: false, session: false };
  } catch (error) {
    return { ok: false, session: error instanceof SessionError };
  }
}

/** Queue a fresh ffprobe inspection of one file. The caller owns the idempotency key. */
export async function queueInspect(api: Api, fileId: string, idempotencyKey: string): Promise<JobResult> {
  try {
    const response = await api.request(`/files/${encodeURIComponent(rawId(fileId))}/inspect`, { method: "POST", headers: { "Idempotency-Key": idempotencyKey } });
    return response.ok ? { ok: true } : { ok: false, session: false };
  } catch (error) {
    return { ok: false, session: error instanceof SessionError };
  }
}

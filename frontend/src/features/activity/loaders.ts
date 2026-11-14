import { SessionError } from "../../lib/api";
import type { EncodeNeed } from "../../lib/encode";
import { jobListParams, type JobCounts, type JobFilter } from "./filters";

/** Data loading for the activity view. UI-free; every loader normalises SessionError. */

export type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

export type JobRow = {
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
  /** Display path of the first file, so a list can name it without a request per job. */
  subject?: string | null;
  /** Whole percent across the job's items, as far as the server can tell. */
  progress?: number | null;
  /** The setting „Delete original“ was on when the job was queued. */
  delete_original?: boolean;
};

export type ItemRow = {
  id: string;
  job_id: string;
  media_id: string;
  action: string;
  position: number;
  status: string;
  attempt: number;
  output_media_id: string | null;
  /** The Kodierfreigabe this item ran with. */
  encode?: string | null;
  /** The Kodierbedarf the server stores for the item's file. */
  encode_need?: EncodeNeed | null;
  display_path?: string | null;
  error_code?: string | null;
  error_detail?: string | null;
  progress?: number | null;
  started_at?: string | null;
};

export type Page<T> = { kind: "ok" | "error" | "session"; items: T[]; nextCursor?: string | undefined };

const apiJobId = (id: string) => encodeURIComponent(decodeURIComponent(id));

async function readPage<T>(api: Api, path: string): Promise<Page<T>> {
  try {
    const response = await api.request(path);
    if (!response.ok) return { kind: "error", items: [] };
    const body = (await response.json()) as { data?: T[]; meta?: { next_cursor?: string | null } };
    return { kind: "ok", items: body.data ?? [], nextCursor: body.meta?.next_cursor ?? undefined };
  } catch (error) {
    return { kind: error instanceof SessionError ? "session" : "error", items: [] };
  }
}

/** One page of the job list for a filter; nothing chosen lists every job status, cancelled ones included. */
export function loadJobs(api: Api, query: { filter: JobFilter; cursor?: string | undefined }): Promise<Page<JobRow>> {
  const cursor = query.cursor ? `&cursor=${encodeURIComponent(query.cursor)}` : "";
  return readPage<JobRow>(api, `/jobs?limit=50&${jobListParams(query.filter)}${cursor}`);
}

const tally = (value: unknown): Record<string, number> | null =>
  value && typeof value === "object" && !Array.isArray(value)
    ? Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === "number"))
    : null;

/**
 * What each status and action would show with every other filter as it is, how many jobs match of how many
 * there are, and how many waiting or running jobs the filter hides. Null without a usable answer; the bar then
 * shows no counts and no hint appears.
 */
export async function loadJobCounts(api: Api, filter: JobFilter): Promise<JobCounts | null> {
  try {
    const response = await api.request(`/jobs/counts?${jobListParams(filter)}`);
    if (!response.ok) return null;
    const data = ((await response.json()) as { data?: Record<string, unknown> }).data;
    const status = tally(data?.status);
    const action = tally(data?.action);
    if (!data || !status || !action || typeof data.matched !== "number" || typeof data.total !== "number") return null;
    return { facets: { status, action }, matched: data.matched, total: data.total, hiddenActive: typeof data.hidden_active === "number" ? data.hidden_active : 0 };
  } catch {
    return null;
  }
}

export type OneJob = { kind: "ok"; data: JobRow } | { kind: "error" } | { kind: "session" };

export async function loadJob(api: Api, id: string): Promise<OneJob> {
  try {
    const response = await api.request(`/jobs/${apiJobId(id)}`);
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: JobRow };
    return body.data ? { kind: "ok", data: body.data } : { kind: "error" };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

export function loadItems(api: Api, id: string, cursor?: string): Promise<Page<ItemRow>> {
  return readPage<ItemRow>(api, `/jobs/${apiJobId(id)}/items?limit=50${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`);
}

export type LogData = { item_id: string; status: string; log_tail: string };
export type LogState = { kind: "ok" | "error" | "session"; data?: LogData };

export async function loadLog(api: Api, itemId: string): Promise<LogState> {
  try {
    const response = await api.request(`/job-items/${encodeURIComponent(itemId)}/log`);
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: LogData };
    return body.data ? { kind: "ok", data: body.data } : { kind: "error" };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

export type MutationResult = { ok: true; id: string | undefined } | { ok: false; session: boolean };

/** Cancel or retry one job. The caller owns the idempotency key so a retried attempt reuses it. */
export async function mutateJob(api: Api, id: string, operation: "cancel" | "retry", idempotencyKey: string): Promise<MutationResult> {
  try {
    const response = await api.request(`/jobs/${apiJobId(id)}/${operation}`, { method: "POST", headers: { "Idempotency-Key": idempotencyKey } });
    if (!response.ok) return { ok: false, session: false };
    const body = (await response.json().catch(() => ({}))) as { data?: { id?: string } };
    return { ok: true, id: body.data?.id };
  } catch (error) {
    return { ok: false, session: error instanceof SessionError };
  }
}

/** Queue a new job for one item whose encoding was not allowed, at a higher level. The caller owns the idempotency key. */
export async function raiseEncode(api: Api, itemId: string, encode: string, idempotencyKey: string): Promise<MutationResult> {
  try {
    const response = await api.request(`/job-items/${apiJobId(itemId)}/raise-encode`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ encode, ...(encode === "all" ? { confirm_video_encode: true } : {}) }),
    });
    if (!response.ok) return { ok: false, session: false };
    const body = (await response.json().catch(() => ({}))) as { data?: { id?: string } };
    return { ok: true, id: body.data?.id };
  } catch (error) {
    return { ok: false, session: error instanceof SessionError };
  }
}

export type ClearStatus = "succeeded" | "partial_success" | "failed" | "cancelled";
export type ClearResult = { ok: true; cleared: number } | { ok: false; session: boolean };

/** Hide finished jobs from Activity. The caller owns the idempotency key so a retried attempt reuses it. */
export async function clearJobs(api: Api, statuses: ClearStatus[], idempotencyKey: string): Promise<ClearResult> {
  try {
    const response = await api.request("/jobs/clear", {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey, "Content-Type": "application/json" },
      body: JSON.stringify({ statuses }),
    });
    if (!response.ok) return { ok: false, session: false };
    const body = (await response.json().catch(() => ({}))) as { data?: { cleared?: number } };
    return { ok: true, cleared: body.data?.cleared ?? 0 };
  } catch (error) {
    return { ok: false, session: error instanceof SessionError };
  }
}

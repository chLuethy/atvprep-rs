import { SessionError } from "../../lib/api";

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
  /** The job carries a one-off confirmation that originals may be deleted. */
  delete_confirmed?: boolean;
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

/** Newest first; `status` is one parent status. Empty means every job except cancelled ones, which have their own filter. */
export function loadJobs(api: Api, query: { status: string; cursor?: string | undefined }): Promise<Page<JobRow>> {
  const parts = ["limit=50", "order=desc"];
  if (query.status) parts.push(`status=${encodeURIComponent(query.status)}`);
  else parts.push("exclude_status=cancelled");
  if (query.cursor) parts.push(`cursor=${encodeURIComponent(query.cursor)}`);
  return readPage<JobRow>(api, `/jobs?${parts.join("&")}`);
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

import { SessionError } from "../../lib/api";

export type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

export type ScanRow = { id: string; root_id: string; generation: number; status: string; started_at: string | null; finished_at: string | null };
export type RootRow = { id: string; name: string; latest_scan: ScanRow | null };
export type RootsResult = { kind: "ok"; items: RootRow[] } | { kind: "error" } | { kind: "session" };

/** The configured libraries by name, each with its latest scan. */
export async function loadRoots(api: Api): Promise<RootsResult> {
  try {
    const response = await api.request("/roots");
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: RootRow[] };
    return { kind: "ok", items: body.data ?? [] };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

export type RemoveOrphansResult =
  | { kind: "ok"; removed: number; bytes: number }
  | { kind: "changed" }
  | { kind: "session" }
  | { kind: "error" };

/**
 * Delete the leftover partial rewrites of one library after the operator confirmed their count and
 * size. The server deletes nothing when the files differ from that. The caller owns the key.
 */
export async function removeOrphans(api: Api, rootId: string, count: number, bytes: number, idempotencyKey: string): Promise<RemoveOrphansResult> {
  try {
    const response = await api.request(`/roots/${encodeURIComponent(rootId)}/orphans/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ count, bytes }),
    });
    if (response.status === 409) return { kind: "changed" };
    if (!response.ok) return { kind: "error" };
    const body = (await response.json().catch(() => ({}))) as { data?: { removed?: number; bytes?: number } };
    return { kind: "ok", removed: body.data?.removed ?? 0, bytes: body.data?.bytes ?? 0 };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

export type ScanResult ={ ok: true } | { ok: false; session: boolean };

/** Queue a manual scan of one library. The caller owns the idempotency key so a retried click reuses it. */
export async function requestScan(api: Api, rootId: string, idempotencyKey: string): Promise<ScanResult> {
  try {
    const response = await api.request("/scans", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey },
      body: JSON.stringify({ root_id: rootId }),
    });
    return response.ok ? { ok: true } : { ok: false, session: false };
  } catch (error) {
    return { ok: false, session: error instanceof SessionError };
  }
}

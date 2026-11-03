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

export type ScanResult = { ok: true } | { ok: false; session: boolean };

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

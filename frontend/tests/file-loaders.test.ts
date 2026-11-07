import { describe, expect, it, vi } from "vitest";
import {
  loadCounts,
  loadDetail,
  loadDetailMounted,
  loadFiles,
  loadInspections,
  loadInspectionsMounted,
  loadJobsMounted,
  loadSidecars,
  queueJob,
} from "../src/features/files/loaders";
import { SessionError } from "../src/lib/api";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };
const apiFor = (handler: (path: string, init?: RequestInit) => Response | Promise<Response>): Api => ({
  request: vi.fn(async (path: string, init?: RequestInit) => handler(path, init)),
});
const expired = () => apiFor(() => Promise.reject(new SessionError()));
const json = (data: unknown, status = 200, meta: unknown = {}) => new Response(JSON.stringify({ data, meta }), { status });

describe("session handling in the file loaders", () => {
  it("lets raw loaders rethrow a lost session so callers can react", async () => {
    await expect(loadDetail(expired(), "m_f1")).rejects.toBeInstanceOf(SessionError);
    await expect(loadInspections(expired(), "m_f1")).rejects.toBeInstanceOf(SessionError);
  });

  it("normalises a lost session for every mounted resource", async () => {
    await expect(loadDetailMounted(expired(), "m_f1")).resolves.toEqual({ kind: "session" });
    await expect(loadJobsMounted(expired(), "m_f1")).resolves.toEqual({ kind: "session", items: [] });
    await expect(loadInspectionsMounted(expired(), "m_f1")).resolves.toEqual({ kind: "session", items: [], error: true });
    await expect(loadSidecars(expired(), "m_f1")).resolves.toEqual({ kind: "session" });
  });

  it("turns other failures into a retryable error, not a session problem", async () => {
    const down = apiFor(() => json({}, 503));
    await expect(loadDetailMounted(down, "m_f1")).resolves.toEqual({ kind: "error" });
    await expect(loadJobsMounted(down, "m_f1")).resolves.toEqual({ kind: "error", items: [] });
  });
});

describe("loadFiles", () => {
  it("sends only the filters that are set and reads the cursor", async () => {
    const api = apiFor(() => json([{ id: "m_f1" }], 200, { next_cursor: "c2" }));

    const page = await loadFiles(api, { q: "Heat", complianceState: "blocker", cursor: "c1" });

    expect(page).toMatchObject({ items: [{ id: "m_f1" }], nextCursor: "c2", error: false, session: false });
    const path = (api.request as ReturnType<typeof vi.fn>).mock.calls[0]![0] as string;
    expect(path).toContain("limit=50");
    expect(path).toContain("q=Heat");
    expect(path).toContain("compliance_state=blocker");
    expect(path).toContain("cursor=c1");
  });

  it("omits empty filters", async () => {
    const api = apiFor(() => json([]));
    await loadFiles(api, { q: "", complianceState: "", cursor: null });
    const path = (api.request as ReturnType<typeof vi.fn>).mock.calls[0]![0] as string;
    expect(path).not.toContain("q=");
    expect(path).not.toContain("compliance_state=");
    expect(path).not.toContain("cursor=");
  });

  it("separates a lost session from a failed request", async () => {
    expect(await loadFiles(expired(), { q: "", complianceState: "", cursor: null })).toMatchObject({ error: true, session: true });
    expect(await loadFiles(apiFor(() => json({}, 500)), { q: "", complianceState: "", cursor: null })).toMatchObject({ error: true, session: false });
  });
});

describe("loadCounts", () => {
  it("returns the per-state file counts and quietly gives null on any failure", async () => {
    expect(await loadCounts(apiFor(() => json({ files: { clean: 3, blocker: 1 } })))).toEqual({ clean: 3, blocker: 1 });
    expect(await loadCounts(apiFor(() => json({}, 503)))).toBeNull();
    expect(await loadCounts(expired())).toBeNull();
  });
});

describe("queueJob", () => {
  it("sends the documented body for a prepare and for a verification, never a fix or convert", async () => {
    const api = apiFor(() => json({ id: "j_1" }, 201));

    await queueJob(api, "prepare", ["m_f1", "m_f2"], "key-1");
    await queueJob(api, "verify", ["m_f1"], "key-2");

    const calls = (api.request as ReturnType<typeof vi.fn>).mock.calls as Array<[string, RequestInit]>;
    expect(calls[0]![0]).toBe("/jobs");
    expect(JSON.parse(String(calls[0]![1].body))).toEqual({ action: "prepare", media_ids: ["m_f1", "m_f2"], manual_approval: true });
    expect(JSON.parse(String(calls[1]![1].body))).toEqual({ action: "verify", media_ids: ["m_f1"], manual_approval: false });
    expect(new Headers(calls[0]![1].headers).get("Idempotency-Key")).toBe("key-1");
    expect(JSON.stringify(calls)).not.toContain("convert");
    expect(JSON.stringify(calls)).not.toContain('"fix"');
  });

  it("sends confirm_reencode only when the caller releases a re-encode", async () => {
    const api = apiFor(() => json({ id: "j_1" }, 201));

    await queueJob(api, "prepare", ["m_f1"], "key-4", { confirmReencode: true });
    await queueJob(api, "prepare", ["m_f1"], "key-5", { confirmReencode: false });

    const calls = (api.request as ReturnType<typeof vi.fn>).mock.calls as Array<[string, RequestInit]>;
    expect(JSON.parse(String(calls[0]![1].body))).toEqual({ action: "prepare", media_ids: ["m_f1"], manual_approval: true, confirm_reencode: true });
    expect(JSON.parse(String(calls[1]![1].body))).toEqual({ action: "prepare", media_ids: ["m_f1"], manual_approval: true });
  });

  it("never asks to delete an original, whatever the caller passes", async () => {
    const api = apiFor(() => json({ id: "j_1" }, 201));

    await queueJob(api, "prepare", ["m_f1"], "key-3");

    const calls = (api.request as ReturnType<typeof vi.fn>).mock.calls as Array<[string, RequestInit]>;
    const body = JSON.parse(String(calls[0]![1].body));
    expect(body).toEqual({ action: "prepare", media_ids: ["m_f1"], manual_approval: true });
    expect(JSON.stringify(body)).not.toContain("delete");
  });

  it("reports failure and distinguishes a lost session", async () => {
    expect(await queueJob(apiFor(() => json({}, 422)), "prepare", ["m_f1"], "k")).toEqual({ ok: false, session: false });
    expect(await queueJob(expired(), "prepare", ["m_f1"], "k")).toEqual({ ok: false, session: true });
    expect(await queueJob(apiFor(() => json({}, 201)), "prepare", ["m_f1"], "k")).toEqual({ ok: true });
  });
});

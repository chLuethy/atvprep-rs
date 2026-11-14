import { describe, expect, it, vi } from "vitest";
import {
  loadCounts,
  loadDetail,
  loadDetailMounted,
  loadFacetCounts,
  loadFiles,
  loadInspections,
  loadInspectionsMounted,
  loadJobsMounted,
  loadSidecars,
  queueJob,
} from "../src/features/files/loaders";
import { EMPTY_FILTER } from "../src/features/files/filters";
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

    const page = await loadFiles(api, { ...EMPTY_FILTER, q: "Heat", status: ["warning", "blocker"], has: ["embed"], cursor: "c1" });

    expect(page).toMatchObject({ items: [{ id: "m_f1" }], nextCursor: "c2", error: false, session: false });
    const path = (api.request as ReturnType<typeof vi.fn>).mock.calls[0]![0] as string;
    expect(path).toContain("limit=50");
    expect(path).toContain("q=Heat");
    expect(path).toContain("status=warning%2Cblocker");
    expect(path).toContain("has=embed");
    expect(path).not.toContain("compliance_state=");
    expect(path).toContain("cursor=c1");
  });

  it("omits empty filters", async () => {
    const api = apiFor(() => json([]));
    await loadFiles(api, { ...EMPTY_FILTER, cursor: null });
    const path = (api.request as ReturnType<typeof vi.fn>).mock.calls[0]![0] as string;
    expect(path).not.toContain("q=");
    expect(path).not.toContain("status=");
    expect(path).not.toContain("has=");
    expect(path).not.toContain("cursor=");
  });

  it("sends library, extension, subtitle files and the sort the way the server takes them", async () => {
    const api = apiFor(() => json([]));
    const filter = { ...EMPTY_FILTER, root: ["r_a", "r_b"], suffix: [".mkv"], sidecar: false };
    await loadFiles(api, { ...filter, cursor: null, sort: { key: "size", dir: -1 } });
    // Recently added is the order without a sort; newest first is only the direction.
    await loadFiles(api, { ...EMPTY_FILTER, cursor: null, sort: { key: "added", dir: -1 } });
    const calls = (api.request as ReturnType<typeof vi.fn>).mock.calls.map((call) => new URLSearchParams(String(call[0]).split("?")[1]));
    expect(calls[0]!.get("root_id")).toBe("r_a,r_b");
    expect(calls[0]!.get("suffix")).toBe(".mkv");
    expect(calls[0]!.get("has_sidecar")).toBe("false");
    expect(calls[0]!.get("sort")).toBe("size");
    expect(calls[0]!.get("dir")).toBe("desc");
    expect(calls[1]!.has("sort")).toBe(false);
    expect(calls[1]!.get("dir")).toBe("desc");
  });

  it("separates a lost session from a failed request", async () => {
    expect(await loadFiles(expired(), { ...EMPTY_FILTER, cursor: null })).toMatchObject({ error: true, session: true });
    expect(await loadFiles(apiFor(() => json({}, 500)), { ...EMPTY_FILTER, cursor: null })).toMatchObject({ error: true, session: false });
  });
});

describe("loadFacetCounts", () => {
  it("asks with the same filters as the list and reads the counts per facet and the totals", async () => {
    const api = apiFor(() => json({ status: { clean: 2, warning: 1 }, has: { embed: 1 }, matched: 1, total: 3 }));

    const counts = await loadFacetCounts(api, { ...EMPTY_FILTER, q: "Heat", status: ["warning"], has: ["embed"] });

    expect(counts).toEqual({ facets: { status: { clean: 2, warning: 1 }, has: { embed: 1 } }, matched: 1, total: 3 });
    expect((api.request as ReturnType<typeof vi.fn>).mock.calls[0]![0]).toBe("/files/counts?status=warning&has=embed&q=Heat");
  });

  it("reads the counts of library, extension and subtitle files when the server gives them", async () => {
    const api = apiFor(() => json({ status: {}, has: {}, root_id: { r_a: 2 }, suffix: { ".mkv": 1, ".mp4": 0 }, has_sidecar: { true: 1, false: 1 }, matched: 2, total: 2 }));
    const counts = await loadFacetCounts(api, { ...EMPTY_FILTER, suffix: [".mkv"], sidecar: true });
    expect(counts?.facets).toEqual({ status: {}, has: {}, root: { r_a: 2 }, suffix: { ".mkv": 1, ".mp4": 0 }, sidecar: { true: 1, false: 1 } });
    expect((api.request as ReturnType<typeof vi.fn>).mock.calls[0]![0]).toBe("/files/counts?suffix=.mkv&has_sidecar=true");
  });

  it("gives null for a failure or an answer of the wrong shape", async () => {
    expect(await loadFacetCounts(apiFor(() => json({}, 503)), EMPTY_FILTER)).toBeNull();
    expect(await loadFacetCounts(apiFor(() => json([])), EMPTY_FILTER)).toBeNull();
    expect(await loadFacetCounts(apiFor(() => json({ status: {}, has: {} })), EMPTY_FILTER)).toBeNull();
    expect(await loadFacetCounts(expired(), EMPTY_FILTER)).toBeNull();
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
  it("asks for a forced rebuild only when told to, and only for a prepare", async () => {
    const api = apiFor(() => json({ id: "j_1" }, 201));

    await queueJob(api, "prepare", ["m_f1"], "key-a", { encode: "copy", force: true });
    await queueJob(api, "prepare", ["m_f1"], "key-b", { encode: "copy", force: false });
    await queueJob(api, "verify", ["m_f1"], "key-c", { force: true });

    const calls = (api.request as ReturnType<typeof vi.fn>).mock.calls as Array<[string, RequestInit]>;
    expect(JSON.parse(String(calls[0]![1].body))).toEqual({ action: "prepare", media_ids: ["m_f1"], manual_approval: true, encode: "copy", force: true });
    expect(JSON.parse(String(calls[1]![1].body))).toEqual({ action: "prepare", media_ids: ["m_f1"], manual_approval: true, encode: "copy" });
    expect(JSON.parse(String(calls[2]![1].body))).toEqual({ action: "verify", media_ids: ["m_f1"], manual_approval: false });
  });

  it("sends the documented body for a prepare and for a verification, never a fix or convert", async () => {
    const api = apiFor(() => json({ id: "j_1" }, 201));

    await queueJob(api, "prepare", ["m_f1", "m_f2"], "key-1");
    await queueJob(api, "verify", ["m_f1"], "key-2");

    const calls = (api.request as ReturnType<typeof vi.fn>).mock.calls as Array<[string, RequestInit]>;
    expect(calls[0]![0]).toBe("/jobs");
    expect(JSON.parse(String(calls[0]![1].body))).toEqual({ action: "prepare", media_ids: ["m_f1", "m_f2"], manual_approval: true, encode: "audio" });
    expect(JSON.parse(String(calls[1]![1].body))).toEqual({ action: "verify", media_ids: ["m_f1"], manual_approval: false });
    expect(new Headers(calls[0]![1].headers).get("Idempotency-Key")).toBe("key-1");
    expect(JSON.stringify(calls)).not.toContain("convert");
    expect(JSON.stringify(calls)).not.toContain('"fix"');
  });

  it("sends the chosen allowance, and the confirmation only with the video level", async () => {
    const api = apiFor(() => json({ id: "j_1" }, 201));

    await queueJob(api, "prepare", ["m_f1"], "key-4", { encode: "all", confirmVideoEncode: true });
    await queueJob(api, "prepare", ["m_f1"], "key-5", { encode: "copy", confirmVideoEncode: true });

    const calls = (api.request as ReturnType<typeof vi.fn>).mock.calls as Array<[string, RequestInit]>;
    expect(JSON.parse(String(calls[0]![1].body))).toEqual({ action: "prepare", media_ids: ["m_f1"], manual_approval: true, encode: "all", confirm_video_encode: true });
    expect(JSON.parse(String(calls[1]![1].body))).toEqual({ action: "prepare", media_ids: ["m_f1"], manual_approval: true, encode: "copy" });
  });

  it("never asks to delete an original, whatever the caller passes", async () => {
    const api = apiFor(() => json({ id: "j_1" }, 201));

    await queueJob(api, "prepare", ["m_f1"], "key-3");

    const calls = (api.request as ReturnType<typeof vi.fn>).mock.calls as Array<[string, RequestInit]>;
    const body = JSON.parse(String(calls[0]![1].body));
    expect(body).toEqual({ action: "prepare", media_ids: ["m_f1"], manual_approval: true, encode: "audio" });
    expect(JSON.stringify(body)).not.toContain("delete");
  });

  it("reports failure and distinguishes a lost session", async () => {
    expect(await queueJob(apiFor(() => json({}, 422)), "prepare", ["m_f1"], "k")).toEqual({ ok: false, session: false });
    expect(await queueJob(expired(), "prepare", ["m_f1"], "k")).toEqual({ ok: false, session: true });
    expect(await queueJob(apiFor(() => json({}, 201)), "prepare", ["m_f1"], "k")).toEqual({ ok: true });
  });
});

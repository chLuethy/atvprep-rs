import { beforeEach, describe, expect, it, vi } from "vitest";
import { ManagerApi, SessionError } from "../src/lib/api";


describe("ManagerApi session client", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
    sessionStorage.clear();
  });

  it("bootstraps with the master token without persisting it", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ csrf_token: "csrf-1" }), { status: 200 }),
    );
    const api = new ManagerApi("/api/v1");

    await api.bootstrap("master-secret");

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/auth/session",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ Authorization: "Bearer master-secret" }),
      }),
    );
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
    expect(api.csrfToken).toBe("csrf-1");
  });

  it("raises a generic session error on bootstrap failure", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("no", { status: 401 }));
    await expect(new ManagerApi().bootstrap("wrong")).rejects.toBeInstanceOf(SessionError);
  });

  it("normalizes encoded mutation paths before issuing CSRF", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      new Response(JSON.stringify({ csrf_token: "csrf-job-cancel" }), { status: 200 }),
    ).mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 }));
    const api = new ManagerApi();

    await api.request("/jobs/j%3A42/cancel", { method: "POST" });

    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/v1/auth/csrf?method=POST&path=%2Fapi%2Fv1%2Fjobs%2Fj%3A42%2Fcancel&action=cancel-job");
  });

  it("issues the clear-jobs CSRF action for clearing finished jobs", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      new Response(JSON.stringify({ csrf_token: "csrf-clear" }), { status: 200 }),
    ).mockResolvedValueOnce(new Response(JSON.stringify({ data: { cleared: 1 } }), { status: 200 }));

    await new ManagerApi().request("/jobs/clear", { method: "POST" });

    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/v1/auth/csrf?method=POST&path=%2Fapi%2Fv1%2Fjobs%2Fclear&action=clear-jobs");
  });

  it("issues the dismiss-missing CSRF action for removing missing files", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      new Response(JSON.stringify({ csrf_token: "csrf-dismiss" }), { status: 200 }),
    ).mockResolvedValueOnce(new Response(JSON.stringify({ data: { removed: 1 } }), { status: 200 }));

    await new ManagerApi().request("/files/dismiss-missing", { method: "POST" });

    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/v1/auth/csrf?method=POST&path=%2Fapi%2Fv1%2Ffiles%2Fdismiss-missing&action=dismiss-missing");
  });

  it("obtains a request-bound CSRF token before each mutation", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      new Response(JSON.stringify({ csrf_token: "csrf-job-create" }), { status: 200 }),
    ).mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 }));
    const api = new ManagerApi();

    await api.request("/jobs", { method: "POST", body: JSON.stringify({ action: "inspect" }) });

    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/v1/auth/csrf?method=POST&path=%2Fapi%2Fv1%2Fjobs&action=create-job");
    const requestHeaders = fetchMock.mock.calls[1]?.[1]?.headers as Headers;
    expect(requestHeaders.get("X-CSRF-Token")).toBe("csrf-job-create");
    expect(requestHeaders.get("Idempotency-Key")).toEqual(expect.any(String));
  });

  it("logs out with CSRF and clears in-memory CSRF state", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 204 }));
    const api = new ManagerApi();
    api.setCsrfToken("csrf-logout");

    await api.logout();

    const headers = fetchMock.mock.calls[0]?.[1]?.headers as Headers;
    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/v1/auth/session");
    expect(headers.get("X-CSRF-Token")).toBe("csrf-logout");
    expect(api.csrfToken).toBeNull();
  });

  it("clears CSRF state when a request returns 401", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 401 }));
    const api = new ManagerApi();
    api.setCsrfToken("expired");

    await expect(api.request("/jobs", { method: "POST" })).rejects.toBeInstanceOf(SessionError);
    expect(api.csrfToken).toBeNull();
  });
});

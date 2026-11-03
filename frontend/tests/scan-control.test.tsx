import { fireEvent, render, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ScanControl } from "../src/features/scan/ScanControl";
import { SessionError } from "../src/lib/api";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };
const json = (data: unknown, status = 200) => new Response(JSON.stringify({ data, meta: {} }), { status });
const scan = (status: string, over: Record<string, unknown> = {}) => ({ id: "s_1", root_id: "r_1", generation: 1, status, started_at: "2026-07-20T10:00:00Z", finished_at: status === "succeeded" ? "2026-07-20T10:05:00Z" : null, ...over });
const root = (id: string, name: string, latest: unknown) => ({ id, name, latest_scan: latest });

function makeApi(state: { roots: unknown[]; post?: (body: { root_id: string }) => Response | Promise<Response> }) {
  const calls: Array<{ path: string; init?: RequestInit }> = [];
  const api: Api = {
    request: vi.fn(async (path: string, init?: RequestInit) => {
      calls.push({ path, init });
      if (path === "/roots") return json(state.roots);
      if (path === "/scans" && init?.method === "POST") return state.post ? state.post(JSON.parse(String(init.body))) : json({ id: "s_9", status: "queued" }, 201);
      return json([]);
    }),
  };
  return { api, posts: () => calls.filter((call) => call.path === "/scans" && call.init?.method === "POST") };
}

afterEach(() => { vi.useRealTimers(); });

describe("ScanControl", () => {
  it("names each library and says when it was last scanned or that it never was", async () => {
    const { api } = makeApi({ roots: [root("r_1", "Film", scan("succeeded")), root("r_2", "TV", null)] });
    const view = render(() => <ScanControl api={api} />);

    expect(await view.findByRole("button", { name: "Scan libraries" })).toBeEnabled();
    const status = view.getByRole("status");
    expect(status).toHaveTextContent(/Film.*scanned/);
    expect(status).toHaveTextContent(/TV.*never scanned/);
  });

  it("uses the singular for a single library", async () => {
    const { api } = makeApi({ roots: [root("r_1", "Film", null)] });
    const view = render(() => <ScanControl api={api} />);

    expect(await view.findByRole("button", { name: "Scan library" })).toBeInTheDocument();
  });

  it("queues one scan per library without a modal and shows them as waiting", async () => {
    const state = {
      roots: [root("r_1", "Film", scan("succeeded")), root("r_2", "TV", scan("succeeded", { id: "s_2", root_id: "r_2" }))] as unknown[],
      post: (_body: { root_id: string }) => {
        state.roots = [root("r_1", "Film", scan("queued")), root("r_2", "TV", scan("queued"))];
        return json({ id: "s_9", status: "queued" }, 201);
      },
    };
    const { api, posts } = makeApi(state);
    const view = render(() => <ScanControl api={api} />);

    await fireEvent.click(await view.findByRole("button", { name: "Scan libraries" }));

    await waitFor(() => expect(posts()).toHaveLength(2));
    expect(posts().map((call) => JSON.parse(String(call.init?.body)))).toEqual([{ root_id: "r_1" }, { root_id: "r_2" }]);
    expect(view.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await view.findByRole("button", { name: "Scanning" })).toBeDisabled();
    expect(view.getByRole("status")).toHaveTextContent(/Film.*waiting to start/);
  });

  it("blocks a second click while the first is in flight", async () => {
    let finish!: (value: Response) => void;
    const { api, posts } = makeApi({ roots: [root("r_1", "Film", null)], post: () => new Promise<Response>((resolve) => { finish = resolve; }) });
    const view = render(() => <ScanControl api={api} />);
    const button = await view.findByRole("button", { name: "Scan library" });

    await fireEvent.click(button);
    await fireEvent.click(button);

    expect(posts()).toHaveLength(1);
    expect(button).toBeDisabled();
    finish(json({ id: "s_9", status: "queued" }, 201));
  });

  it("follows a running scan, disables the button meanwhile and tells the page when it finishes", async () => {
    vi.useFakeTimers();
    let status = "running";
    const onFinished = vi.fn();
    const api: Api = { request: vi.fn(async () => json([root("r_1", "Film", scan(status))])) };
    const view = render(() => <ScanControl api={api} onFinished={onFinished} />);
    await vi.advanceTimersByTimeAsync(0);
    expect(view.getByRole("button", { name: "Scanning" })).toBeDisabled();
    expect(view.getByRole("status")).toHaveTextContent(/Film.*scanning/);
    expect(onFinished).not.toHaveBeenCalled();

    status = "succeeded";
    await vi.advanceTimersByTimeAsync(4100);

    expect(view.getByRole("button", { name: "Scan library" })).toBeEnabled();
    expect(onFinished).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(12000);
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it("reports a failed request and an expired session in plain words", async () => {
    const failing = makeApi({ roots: [root("r_1", "Film", null)], post: () => json({}, 500) });
    const first = render(() => <ScanControl api={failing.api} />);
    await fireEvent.click(await first.findByRole("button", { name: "Scan library" }));
    expect(await first.findByRole("alert")).toHaveTextContent(/unable to queue the scan/i);
    first.unmount();

    const expired = makeApi({ roots: [root("r_1", "Film", null)], post: () => { throw new SessionError(); } });
    const second = render(() => <ScanControl api={expired.api} />);
    await fireEvent.click(await second.findByRole("button", { name: "Scan library" }));
    expect(await second.findByRole("alert")).toHaveTextContent(/session expired/i);
  });

  it("stays silent when the session expired while loading, because the page already says so", async () => {
    const api: Api = { request: vi.fn(async () => { throw new SessionError(); }) };
    const view = render(() => <ScanControl api={api} />);
    await waitFor(() => expect(api.request).toHaveBeenCalled());

    expect(view.queryByRole("alert")).not.toBeInTheDocument();
    expect(view.queryByRole("button")).not.toBeInTheDocument();
  });

  it("explains how a library gets set up when none is known", async () => {
    const { api } = makeApi({ roots: [] });
    const view = render(() => <ScanControl api={api} />);

    expect(await view.findByText(/no library is set up/i)).toBeInTheDocument();
    expect(view.getByText(/roots/)).toBeInTheDocument();
    expect(view.queryByRole("button")).not.toBeInTheDocument();
  });

  it("says so when the libraries cannot be loaded", async () => {
    const api: Api = { request: vi.fn(async () => json({}, 503)) };
    const view = render(() => <ScanControl api={api} />);

    expect(await view.findByRole("alert")).toHaveTextContent(/unable to load libraries/i);
  });
});

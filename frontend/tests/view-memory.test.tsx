import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../src/App";

// The files and activity pages keep their filter, search and order across a page change, a reload and a new tab.
const real = window.localStorage;

describe("filters survive a page change", () => {
  class QuietSocket { static OPEN = 1; readyState = 0; addEventListener() {} removeEventListener() {} close() {} send() {} }
  beforeEach(() => {
    vi.stubGlobal("WebSocket", QuietSocket);
    window.history.replaceState(null, "", "/");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    window.localStorage.clear();
    window.history.replaceState(null, "", "/");
  });

  const fakeApi = (refuse?: (path: string) => boolean) => {
    const paths: string[] = [];
    const api = {
      request: vi.fn(async (path: string) => {
        paths.push(path);
        if (refuse?.(path)) return new Response(JSON.stringify({ error: { code: "invalid_filter", message: "unknown library" } }), { status: 400 });
        if (path.startsWith("/files?") || path.startsWith("/jobs?")) return new Response(JSON.stringify({ data: [], meta: { next_cursor: null } }));
        if (path === "/roots") return new Response(JSON.stringify({ data: [] }));
        return new Response(JSON.stringify({ data: {} }));
      }),
    };
    const last = (prefix: string) => paths.filter((path) => path.startsWith(prefix)).at(-1) ?? "";
    return { api, lastFiles: () => last("/files?"), lastJobs: () => last("/jobs?") };
  };

  const nav = (view: ReturnType<typeof render>, name: RegExp) =>
    within(view.getByRole("navigation", { name: /main/i })).getByRole("link", { name });

  it("brings back the files filter, search and order after a visit to Activity", async () => {
    window.history.replaceState(null, "", "/files?status=blocker&q=dune&sort=size&dir=desc");
    const { api, lastFiles } = fakeApi();
    const view = render(() => <App api={api as never} />);
    await waitFor(() => expect(lastFiles()).toContain("status=blocker"));

    await fireEvent.click(nav(view, /Activity/));
    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
    await fireEvent.click(nav(view, /Files/));

    await waitFor(() => expect(window.location.search).toBe("?status=blocker&q=dune&sort=size&dir=desc"));
    await waitFor(() => expect(lastFiles()).toContain("q=dune"));
    expect(lastFiles()).toContain("sort=size");
    expect(await view.findByRole("combobox", { name: "Sort by" })).toHaveValue("size");
  });
  it("brings back the activity filter, search, order and period after a visit to Files", async () => {
    window.history.replaceState(null, "", "/jobs?status=failed&action=prepare&q=heat&range=7d&sort=oldest");
    const { api, lastJobs } = fakeApi();
    const view = render(() => <App api={api as never} />);
    await waitFor(() => expect(lastJobs()).toContain("status=failed"));

    await fireEvent.click(nav(view, /Files/));
    await waitFor(() => expect(window.location.pathname).toBe("/files"));
    await fireEvent.click(nav(view, /Activity/));

    await waitFor(() => expect(window.location.search).toBe("?status=failed&action=prepare&q=heat&range=7d&sort=oldest"));
    await waitFor(() => expect(lastJobs()).toContain("order=asc"));
    expect(lastJobs()).toContain("q=heat");
    expect(lastJobs()).toContain("from=");
  });

  it("brings the state back after a reload or in a new tab, without a new history entry", async () => {
    window.history.replaceState(null, "", "/files?status=warning&q=alien");
    const first = render(() => <App api={fakeApi().api as never} />);
    await waitFor(() => expect(window.location.search).toBe("?status=warning&q=alien"));
    first.unmount();

    window.history.replaceState(null, "", "/files");
    const before = window.history.length;
    const { api, lastFiles } = fakeApi();
    render(() => <App api={api as never} />);
    await waitFor(() => expect(window.location.search).toBe("?status=warning&q=alien"));
    await waitFor(() => expect(lastFiles()).toContain("q=alien"));
    expect(window.history.length).toBe(before);
  });

  it("lets a link with a filter of its own win and keeps it as the new state", async () => {
    window.history.replaceState(null, "", "/files?q=alien");
    const view = render(() => <App api={fakeApi().api as never} />);
    await waitFor(() => expect(window.location.search).toBe("?q=alien"));

    await fireEvent.keyDown(document, { key: "k", metaKey: true });
    const search = view.getByRole("combobox", { name: "Search commands" });
    await fireEvent.input(search, { target: { value: "Files: Not verified" } });
    await fireEvent.keyDown(search, { key: "Enter" });
    await waitFor(() => expect(window.location.search).toBe("?status=unknown"));

    await fireEvent.click(nav(view, /Activity/));
    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
    await fireEvent.click(nav(view, /Files/));
    await waitFor(() => expect(window.location.search).toBe("?status=unknown"));
  });

  it("forgets the state with Reset filters, so the next visit shows everything", async () => {
    window.history.replaceState(null, "", "/files?status=blocker&q=dune");
    const view = render(() => <App api={fakeApi().api as never} />);
    await fireEvent.click((await view.findAllByRole("button", { name: "Reset filters" }))[0]!);
    await waitFor(() => expect(window.location.search).toBe(""));

    await fireEvent.click(nav(view, /Activity/));
    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
    await fireEvent.click(nav(view, /Files/));
    await waitFor(() => expect(window.location.pathname).toBe("/files"));
    expect(window.location.search).toBe("");
  });

  it("keeps Back within the page undoing the last filter change", async () => {
    window.history.replaceState(null, "", "/files?status=blocker");
    const view = render(() => <App api={fakeApi().api as never} />);
    await waitFor(() => expect(window.location.search).toBe("?status=blocker"));
    await fireEvent.click(nav(view, /Activity/));
    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
    await fireEvent.click(nav(view, /Files/));
    await waitFor(() => expect(window.location.search).toBe("?status=blocker"));

    const bar = await view.findByRole("search", { name: "Filter files" });
    await fireEvent.click(within(bar).getByRole("button", { name: "Status: Blocker" }));
    await fireEvent.click(within(view.getByRole("dialog", { name: "Status" })).getByRole("checkbox", { name: /^Warning/ }));
    await waitFor(() => expect(window.location.search).toBe("?status=warning,blocker"));
    window.history.back();
    await waitFor(() => expect(window.location.search).toBe("?status=blocker"));
    window.history.back();
    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
  });

  it("shows the known error with Reset filters for a remembered filter the server refuses", async () => {
    window.localStorage.clear();
    window.history.replaceState(null, "", "/files?root_id=gone");
    const first = render(() => <App api={fakeApi().api as never} />);
    await waitFor(() => expect(window.location.search).toBe("?root_id=gone"));
    first.unmount();

    window.history.replaceState(null, "", "/files");
    const view = render(() => <App api={fakeApi((path) => path.startsWith("/files?") && path.includes("root_id=gone")).api as never} />);
    const alert = await view.findByRole("alert");
    await fireEvent.click(within(alert).getByRole("button", { name: "Reset filters" }));
    await waitFor(() => expect(window.location.search).toBe(""));
    await waitFor(() => expect(view.queryByRole("alert")).not.toBeInTheDocument());
  });

  it("does not bring back the open file or job, only the filter", async () => {
    window.history.replaceState(null, "", "/files/f_1?status=warning");
    const files = render(() => <App api={fakeApi().api as never} />);
    await fireEvent.click(nav(files, /Activity/));
    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
    await fireEvent.click(nav(files, /Files/));
    await waitFor(() => expect(window.location.search).toBe("?status=warning"));
    expect(window.location.pathname).toBe("/files");
    files.unmount();

    window.history.replaceState(null, "", "/jobs/j_1?status=failed");
    const jobs = render(() => <App api={fakeApi().api as never} />);
    await fireEvent.click(nav(jobs, /Files/));
    await waitFor(() => expect(window.location.pathname).toBe("/files"));
    await fireEvent.click(nav(jobs, /Activity/));
    await waitFor(() => expect(window.location.search).toBe("?status=failed"));
    expect(window.location.pathname).toBe("/jobs");
  });

  it("works as before when the browser has no storage", async () => {
    const broken = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("denied"); }, removeItem: () => { throw new Error("denied"); }, clear: () => {}, key: () => null, length: 0 };
    vi.stubGlobal("localStorage", broken);
    Object.defineProperty(window, "localStorage", { value: broken, configurable: true });
    try {
      window.history.replaceState(null, "", "/files?status=blocker");
      const { api, lastFiles } = fakeApi();
      const view = render(() => <App api={api as never} />);
      await waitFor(() => expect(lastFiles()).toContain("status=blocker"));
      await fireEvent.click(nav(view, /Activity/));
      await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
      await fireEvent.click(nav(view, /Files/));
      await waitFor(() => expect(window.location.pathname).toBe("/files"));
      expect(window.location.search).toBe("");
      await waitFor(() => expect(lastFiles()).not.toContain("status=blocker"));
    } finally {
      Object.defineProperty(window, "localStorage", { value: real, configurable: true });
    }
  });
});

import { fireEvent, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@solidjs/testing-library";
import App from "../src/App";
import { publishWork } from "../src/lib/work";


describe("application shell version", () => {
  // With an api the shell also opens its live connection; jsdom has no real one.
  class QuietSocket { static OPEN = 1; readyState = 0; addEventListener() {} removeEventListener() {} close() {} send() {} }
  beforeEach(() => vi.stubGlobal("WebSocket", QuietSocket));
  afterEach(() => vi.unstubAllGlobals());

  const withVersion = (answer: () => Promise<Response>) => ({ request: vi.fn(async (path: string) => (path === "/version" ? answer() : new Response(JSON.stringify({ data: [] })))) });

  it("shows the running release at the foot of the sidebar", async () => {
    const api = withVersion(async () => new Response(JSON.stringify({ service: "atvprep-manager", version: "0.5.2" })));
    const view = render(() => <App api={api as never} />);

    expect(await view.findByText("Version 0.5.2")).toBeInTheDocument();
    expect(api.request).toHaveBeenCalledWith("/version");
  });

  it("leaves the line out when the version cannot be read", async () => {
    const api = withVersion(async () => new Response("{}", { status: 503 }));
    const view = render(() => <App api={api as never} />);

    await waitFor(() => expect(api.request).toHaveBeenCalledWith("/version"));
    expect(view.queryByText(/^Version /)).not.toBeInTheDocument();
  });
});

describe("application shell accessibility", () => {
  it("provides a skip link and a labelled main landmark", () => {
    const { getByRole } = render(() => <App />);

    expect(getByRole("link", { name: "Skip to content" })).toHaveAttribute("href", "#main-content");
    expect(getByRole("main")).toHaveAttribute("id", "main-content");
  });

  it("marks Activity in the sidebar while the worker has jobs, and only then", () => {
    publishWork({ running: 0, queued: 0 });
    const { getByRole, queryByRole } = render(() => <App />);
    const activity = () => getByRole("link", { name: /Activity/ });
    expect(queryByRole("img", { name: /job/ })).toBeNull();

    publishWork({ running: 2, queued: 1 });
    expect(activity()).toContainElement(getByRole("img", { name: "2 jobs running" }));

    publishWork({ running: 0, queued: 1 });
    expect(activity()).toContainElement(getByRole("img", { name: "1 job waiting" }));

    publishWork({ running: 0, queued: 0 });
    expect(queryByRole("img", { name: /job/ })).toBeNull();
  });

  it("provides an explicit mobile navigation toggle", () => {
    const { getByRole } = render(() => <App />);

    const toggle = getByRole("button", { name: "Open navigation" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveAttribute("aria-controls", "main-navigation");
  });

  it("labels the primary navigation and offers the appearance switch", () => {
    const { getByRole } = render(() => <App />);

    expect(getByRole("navigation", { name: "Main" })).toBeInTheDocument();
    expect(getByRole("group", { name: "Appearance" })).toBeInTheDocument();
  });

  it("opens the navigation from the mobile toggle", async () => {
    const { getByRole } = render(() => <App />);

    await fireEvent.click(getByRole("button", { name: "Open navigation" }));

    expect(getByRole("button", { name: "Close navigation" })).toHaveAttribute("aria-expanded", "true");
  });

  it("shows a Lock control only when the app runs inside a session, and locks through it", async () => {
    const withoutSession = render(() => <App />);
    expect(withoutSession.queryByRole("button", { name: "Lock" })).not.toBeInTheDocument();
    withoutSession.unmount();

    const lock = vi.fn();
    const view = render(() => <App session={{ lock, busy: () => false }} />);
    await fireEvent.click(view.getByRole("button", { name: "Lock" }));
    expect(lock).toHaveBeenCalledOnce();
  });
});

describe("command palette in the shell", () => {
  afterEach(() => {
    document.documentElement.removeAttribute("data-theme");
    window.localStorage.clear();
    window.history.replaceState(null, "", "/");
  });

  const openPalette = async (view: ReturnType<typeof render>) => {
    await fireEvent.keyDown(document, { key: "k", metaKey: true });
    return view.getByRole("combobox", { name: "Search commands" });
  };

  it("opens with Cmd+K or Ctrl+K and closes with Escape", async () => {
    const view = render(() => <App />);

    const search = await openPalette(view);
    expect(view.getByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
    await fireEvent.keyDown(search, { key: "Escape" });
    expect(view.queryByRole("dialog")).not.toBeInTheDocument();

    await fireEvent.keyDown(document, { key: "k", ctrlKey: true });
    expect(view.getByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
  });

  it("moves to a page by name", async () => {
    const view = render(() => <App />);
    const search = await openPalette(view);

    await fireEvent.input(search, { target: { value: "activity" } });
    await fireEvent.keyDown(search, { key: "Enter" });

    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
  });

  it("opens the files page on a chosen status through the address", async () => {
    const view = render(() => <App />);
    const search = await openPalette(view);

    await fireEvent.input(search, { target: { value: "blocker" } });
    await fireEvent.keyDown(search, { key: "Enter" });

    await waitFor(() => expect(window.location.pathname).toBe("/files"));
    expect(window.location.search).toBe("?status=blocker");
  });

  it("offers the quick filters of the files page through the same address", async () => {
    const view = render(() => <App />);
    const search = await openPalette(view);

    await fireEvent.input(search, { target: { value: "needs attention" } });
    await fireEvent.keyDown(search, { key: "Enter" });

    await waitFor(() => expect(window.location.pathname).toBe("/files"));
    expect(window.location.search).toBe("?status=blocker,probe_failed,missing");
  });

  it("offers the quick filters of the activity through the same address", async () => {
    const view = render(() => <App />);
    const search = await openPalette(view);

    await fireEvent.input(search, { target: { value: "failed in 24" } });
    await fireEvent.keyDown(search, { key: "Enter" });

    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
    expect(window.location.search).toBe("?status=partial_success,failed&range=24h");
  });

  it("changes the theme and keeps the appearance switch in step", async () => {
    const view = render(() => <App />);
    const search = await openPalette(view);

    await fireEvent.input(search, { target: { value: "dark" } });
    await fireEvent.keyDown(search, { key: "Enter" });

    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    await waitFor(() => expect(view.getByRole("button", { name: "Dark" })).toHaveAttribute("aria-pressed", "true"));
  });

  it("offers Lock only inside a session, and locks through it", async () => {
    const outside = render(() => <App />);
    const search = await openPalette(outside);
    await fireEvent.input(search, { target: { value: "lock" } });
    expect(outside.queryByRole("option", { name: /Lock manager/ })).not.toBeInTheDocument();
    outside.unmount();

    const lock = vi.fn();
    const inside = render(() => <App session={{ lock, busy: () => false }} />);
    const insideSearch = await openPalette(inside);
    await fireEvent.input(insideSearch, { target: { value: "lock" } });
    await fireEvent.keyDown(insideSearch, { key: "Enter" });
    expect(lock).toHaveBeenCalledOnce();
  });
});

describe("the files filter in the address", () => {
  class QuietSocket { static OPEN = 1; readyState = 0; addEventListener() {} removeEventListener() {} close() {} send() {} }
  beforeEach(() => vi.stubGlobal("WebSocket", QuietSocket));
  afterEach(() => { vi.unstubAllGlobals(); window.history.replaceState(null, "", "/"); });

  const filesApi = () => {
    const paths: string[] = [];
    const api = {
      request: vi.fn(async (path: string) => {
        paths.push(path);
        if (path.startsWith("/files?")) return new Response(JSON.stringify({ data: [], meta: { next_cursor: null } }));
        if (path === "/roots") return new Response(JSON.stringify({ data: [] }));
        return new Response(JSON.stringify({ data: {} }));
      }),
    };
    return { api, lastList: () => paths.filter((path) => path.startsWith("/files?")).at(-1) ?? "" };
  };

  it("keeps the sort in the address after the filter; a new order is a history entry", async () => {
    window.history.replaceState(null, "", "/files?dir=desc&sort=size&has_sidecar=true");
    const { api, lastList } = filesApi();
    const view = render(() => <App api={api as never} />);

    await waitFor(() => expect(window.location.search).toBe("?has_sidecar=true&sort=size&dir=desc"));
    await waitFor(() => expect(lastList()).toContain("sort=size"));
    expect(lastList()).toContain("has_sidecar=true");
    const select = await view.findByRole("combobox", { name: "Sort by" });
    await fireEvent.change(select, { target: { value: "added" } });
    await waitFor(() => expect(window.location.search).toBe("?has_sidecar=true&sort=added&dir=desc"));
    window.history.back();
    await waitFor(() => expect(select).toHaveValue("size"));
  });

  it("rewrites an old address with hide to the states it showed, without a new history entry", async () => {
    window.history.replaceState(null, "", "/files?hide=clean&has=embed");
    const before = window.history.length;
    const { api, lastList } = filesApi();
    render(() => <App api={api as never} />);

    await waitFor(() => expect(window.location.search).toBe("?status=embed_open,warning,blocker,unknown,probe_failed,missing&has=embed"));
    expect(window.history.length).toBe(before);
    await waitFor(() => expect(lastList()).toContain("status=embed_open%2Cwarning%2Cblocker%2Cunknown%2Cprobe_failed%2Cmissing"));
    expect(lastList()).not.toContain("hide=");
  });

  it("adds a history entry for a chosen state, and Back brings the earlier filter back", async () => {
    window.history.replaceState(null, "", "/files?status=blocker");
    const { api, lastList } = filesApi();
    const view = render(() => <App api={api as never} />);
    const bar = await view.findByRole("search", { name: "Filter files" });
    await fireEvent.click(within(bar).getByRole("button", { name: "Status: Blocker" }));
    await fireEvent.click(within(view.getByRole("dialog", { name: "Status" })).getByRole("checkbox", { name: /^Warning/ }));

    await waitFor(() => expect(window.location.search).toBe("?status=warning,blocker"));
    window.history.back();
    await waitFor(() => expect(window.location.search).toBe("?status=blocker"));
    await waitFor(() => expect(within(bar).getByRole("button", { name: "Status: Blocker" })).toBeInTheDocument());
    await waitFor(() => expect(lastList()).toMatch(/status=blocker(&|$)/));
  });
});

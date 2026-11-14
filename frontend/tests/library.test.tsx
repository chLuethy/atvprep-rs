import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LibraryPage } from "../src/features/files/LibraryPage";
import { SessionError } from "../src/lib/api";
import { setLanguage } from "../src/lib/i18n";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

const GB = 1024 ** 3;
type Row = Record<string, unknown> & { id: string };
const file = (over: Row): Row => ({
  root_id: "r_1", kind: "movie", suffix: ".mp4", mtime_ns: 1, discovery_state: "present",
  workflow_state: "idle", desired_action: "none", last_seen_at: null, compliance_reasons: [], ...over,
});

const ROWS = [
  file({ id: "m_f1", display_path: "Film/Pretty Woman (1990)/Pretty Woman (1990).mp4", size: 10 * GB, compliance_state: "warning", desired_action: "fix",
    compliance_reasons: ["[warning] apple-tags: 4 tracks lack an elng tag"] }),
  file({ id: "m_f2", display_path: "Film/Heat (1995)/Heat.mp4", size: 8 * GB, compliance_state: "blocker", desired_action: "fix",
    compliance_reasons: ["[blocker] ftyp-dby1-requires-atmos: ftyp lists dby1 but no track is Atmos"] }),
  file({ id: "m_f3", display_path: "Film/Arrival (2016)/Arrival (2016).mp4", size: 7 * GB, compliance_state: "clean",
    compliance_reasons: ["No compliance violations reported by verify."] }),
  file({ id: "m_f4", display_path: "Eingang/Whiplash (2014)/Whiplash (2014).mp4", size: 5 * GB, compliance_state: "unknown",
    compliance_reasons: ["Current ffprobe inspection is available, but AVPlayer compliance requires a verify report."] }),
];

const json = (data: unknown, status = 200, meta: unknown = {}) => new Response(JSON.stringify({ data, meta }), { status });

let lastCalls: Array<{ path: string; init?: RequestInit }> = [];
const calls = (_api: Api) => lastCalls;

function makeApi(options: { rows?: unknown[]; next?: string | null; more?: unknown[]; fail?: boolean; counts?: unknown } = {}) {
  const calls: Array<{ path: string; init?: RequestInit }> = [];
  lastCalls = calls;
  const api: Api = {
    request: vi.fn(async (path: string, init?: RequestInit) => {
      calls.push({ path, init });
      if (path === "/jobs") return json({ id: "j_1" }, 201);
      if (path === "/dashboard") return json({ files: { clean: 1, warning: 1, blocker: 1, unknown: 1 }, scans: {}, jobs: {}, delete_original: false });
      if (path.startsWith("/files/counts")) return options.counts ? json(options.counts) : json({}, 503);
      if (path.startsWith("/files?")) {
        if (options.fail) return json({}, 503);
        if (path.includes("cursor=c2")) return json(options.more ?? [], 200, { next_cursor: null });
        return json(options.rows ?? ROWS, 200, { next_cursor: options.next ?? null });
      }
      if (/^\/files\/m_f\d$/.test(path)) return json({ ...ROWS.find((row) => path.endsWith(String(row.id))), status_version: 1, last_success_at: null, has_error: false, sidecar_count: 0, inspection_available: true });
      return json([]);
    }),
  };
  return { api, calls, posts: () => calls.filter((call) => call.path === "/jobs") };
}


/** The row of a file by the name it shows. */
const rowNamed = (view: { getByRole: (role: string, options: { name: string }) => HTMLElement }, name: string) =>
  view.getByRole("link", { name }).closest("tr") as HTMLElement;
/** Add a file to the selection the way a person does: Cmd-click its row. */
const pickRow = async (view: { findByRole: (role: string, options: { name: string }) => Promise<HTMLElement> }, name: string) =>
  fireEvent.click(((await view.findByRole("link", { name })).closest("tr")) as HTMLElement, { metaKey: true });
const isPicked = (view: { getByRole: (role: string, options: { name: string }) => HTMLElement }, name: string) =>
  rowNamed(view, name).getAttribute("data-picked") === "true";

const open = vi.fn();
const renderLibrary = (api: Api, activeId?: string) => render(() => <LibraryPage api={api} activeId={activeId} onOpen={open} />);

describe("LibraryPage: the page title", () => {
  afterEach(() => setLanguage("en"));

  it("is called Files, and Dateien in German, never Library", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    expect(await view.findByRole("heading", { level: 1, name: "Files" })).toBeInTheDocument();
    expect(view.getByRole("region", { name: "Files" })).toBeInTheDocument();

    setLanguage("de");
    expect(await view.findByRole("heading", { level: 1, name: "Dateien" })).toBeInTheDocument();
    expect(view.queryByText(/Mediathek|Bibliothek/)).not.toBeInTheDocument();
  });
});

describe("LibraryPage: the list", () => {
  it("shows each file with a worded verdict and its first reason, and nothing redundant for clean files", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);

    expect(await view.findByRole("link", { name: "Pretty Woman (1990)" })).toHaveAttribute("href", "/files/m_f1");
    const table = view.getByRole("table", { name: "Media files" });
    expect(table).toHaveTextContent("Warning");
    expect(table).toHaveTextContent("Blocker");
    expect(table).toHaveTextContent("Ready");
    expect(table).toHaveTextContent("Not verified");
    expect(table).toHaveTextContent("4 tracks lack an elng tag");
    expect(table).toHaveTextContent("ftyp lists dby1 but no track is Atmos");
    expect(table).not.toHaveTextContent("No compliance violations reported by verify");
    expect(table).toHaveTextContent("10 GB");
  });

  it("marks a file whose last preparation was interrupted in the list", async () => {
    const rows = [
      file({ id: "m_f1", display_path: "Film/A/A.mp4", size: GB, compliance_state: "warning", last_interrupted: true }),
      file({ id: "m_f2", display_path: "Film/B/B.mp4", size: GB, compliance_state: "warning", last_interrupted: false }),
    ];
    const { api } = makeApi({ rows });
    const view = renderLibrary(api);

    await view.findByRole("link", { name: "A" });
    expect(rowNamed(view, "A")).toHaveTextContent("Last preparation interrupted");
    expect(rowNamed(view, "B")).not.toHaveTextContent("Last preparation interrupted");
  });

  it("opens a file in the inspector when its row or title is chosen", async () => {
    open.mockClear();
    const { api } = makeApi();
    const view = renderLibrary(api);

    await fireEvent.click(await view.findByRole("link", { name: "Heat" }));
    expect(open).toHaveBeenCalledWith("m_f2");
  });

  it("shows the inspector for the active file and a prompt when none is chosen", async () => {
    const { api } = makeApi();
    const none = renderLibrary(api);
    expect(await none.findByText(/select a file to inspect/i)).toBeInTheDocument();
    none.unmount();

    const active = renderLibrary(api, "m_f1");
    expect(await active.findByRole("heading", { name: "Pretty Woman (1990)" })).toBeInTheDocument();
  });

  it("asks the server to sort the whole list by a column header, both ways", async () => {
    const { api, calls } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });

    await fireEvent.click(view.getByRole("button", { name: "File" }));
    await vi.waitFor(() => expect(calls.some((call) => call.path.includes("sort=title") && call.path.includes("dir=asc"))).toBe(true));
    await fireEvent.click(view.getByRole("button", { name: /File/ }));
    await vi.waitFor(() => expect(calls.some((call) => call.path.includes("sort=title") && call.path.includes("dir=desc"))).toBe(true));
  });

  it("explains an empty result and offers a retry after a failed request", async () => {
    const empty = renderLibrary(makeApi({ rows: [] }).api);
    expect(await empty.findByText(/no files match/i)).toBeInTheDocument();
    empty.unmount();

    const failed = renderLibrary(makeApi({ fail: true }).api);
    expect(await failed.findByRole("alert")).toHaveTextContent("Unable to load files");
    expect(failed.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("reports an expired session instead of a generic failure", async () => {
    const api: Api = { request: vi.fn(async (path: string) => { if (path === "/dashboard") return json({}); throw new SessionError(); }) };
    const view = renderLibrary(api);
    expect(await view.findByRole("alert")).toHaveTextContent(/session expired.*lock.*unlock/i);
  });
});

/** A counts answer as the server gives it; anything left out is 0. */
const facetCounts = (over: { status?: Record<string, number>; has?: Record<string, number>; matched?: number; total?: number } = {}) => ({
  status: { clean: 0, embed_open: 0, warning: 0, blocker: 0, unknown: 0, probe_failed: 0, missing: 0, ...over.status },
  has: { convert: 0, encode_audio: 0, encode_video: 0, embed: 0, converted: 0, ...over.has },
  matched: over.matched ?? 4,
  total: over.total ?? 4,
});
const COUNTS = facetCounts({ status: { clean: 1, warning: 1, blocker: 1, unknown: 1 }, has: { encode_video: 2, embed: 1 } });
const lastPath = (prefix: string) => calls({} as Api).filter((call) => call.path.startsWith(prefix)).at(-1)?.path ?? "";
const lastList = () => lastPath("/files?");
const lastCounts = () => lastPath("/files/counts");
/** Open a facet's popover by the title on its button. */
const openFacet = async (view: ReturnType<typeof renderLibrary>, title: string) => {
  const bar = within(await view.findByRole("search", { name: "Filter files" }));
  await fireEvent.click(await bar.findByRole("button", { name: new RegExp(`^${title}(: .+)?$`) }));
  return within(view.getByRole("dialog", { name: title }));
};

describe("LibraryPage: the filter bar", () => {
  it("starts unfiltered: no state or property in the request, plain facet buttons and no reset", async () => {
    const { api } = makeApi({ counts: COUNTS });
    const view = renderLibrary(api);

    expect(await view.findByRole("button", { name: "Status" })).toHaveAttribute("aria-expanded", "false");
    expect(view.getByRole("button", { name: "Properties" })).toBeInTheDocument();
    expect(view.queryByRole("button", { name: "Reset filters" })).not.toBeInTheDocument();
    expect(view.queryByRole("checkbox")).not.toBeInTheDocument();
    await waitFor(() => expect(lastList()).not.toBe(""));
    expect(lastList()).not.toMatch(/status=|has=|hide=|compliance_state=/);
  });

  it("says how many of all files are left", async () => {
    const { api } = makeApi({ counts: facetCounts({ matched: 2, total: 6 }) });
    const view = renderLibrary(api);
    expect(await view.findByText("2 of 6 files")).toHaveAttribute("aria-live", "polite");
  });

  it("shows the files of the chosen states, any of them, and nothing chosen shows all", async () => {
    const { api } = makeApi({ counts: COUNTS });
    const view = renderLibrary(api);
    const status = await openFacet(view, "Status");

    await fireEvent.click(await status.findByRole("checkbox", { name: "Warning 1" }));
    await waitFor(() => expect(lastList()).toContain("status=warning"));
    await fireEvent.click(status.getByRole("checkbox", { name: "Blocker 1" }));
    await waitFor(() => expect(lastList()).toContain("status=warning%2Cblocker"));
    expect(lastCounts()).toContain("status=warning%2Cblocker");
    expect(view.getByRole("button", { name: "Status: Warning, Blocker" })).toBeInTheDocument();

    await fireEvent.click(view.getByRole("button", { name: "Clear Status" }));
    await waitFor(() => expect(lastList()).not.toContain("status="));
  });

  it("offers every state with its own count, Embedding open and Missing included", async () => {
    const { api } = makeApi({ counts: facetCounts({ status: { clean: 1, embed_open: 2, missing: 1 } }) });
    const view = renderLibrary(api);
    const status = await openFacet(view, "Status");
    expect(await status.findByRole("checkbox", { name: "Embedding open 2" })).not.toBeChecked();
    expect(status.getByRole("checkbox", { name: "Ready 1" })).toBeInTheDocument();
    expect(status.getByRole("checkbox", { name: "Missing 1" })).toBeInTheDocument();
    // No hit: dimmed, but it can still be chosen.
    expect(status.getByRole("checkbox", { name: "Warning 0" })).toBeEnabled();
  });

  it("shows only files with all chosen properties, as one has list", async () => {
    const { api } = makeApi({ counts: COUNTS });
    const view = renderLibrary(api);
    const properties = await openFacet(view, "Properties");

    await fireEvent.click(properties.getByRole("checkbox", { name: /^Subtitles embeddable/ }));
    await waitFor(() => expect(lastList()).toContain("has=embed"));
    await fireEvent.click(properties.getByRole("checkbox", { name: /^Needs video encoding/ }));
    await waitFor(() => expect(lastList()).toContain("has=encode_video%2Cembed"));
    expect(properties.getByText("All of these")).toBeInTheDocument();
  });

  it("asks the server for the counts with every filter, and shows what it answers", async () => {
    const { api } = makeApi();
    (api.request as ReturnType<typeof vi.fn>).mockImplementation(async (path: string) => {
      lastCalls.push({ path });
      if (path === "/dashboard") return json({ files: { clean: 1 }, scans: {}, jobs: {} });
      if (path.startsWith("/files/counts")) {
        const has = new URL(path, "http://x").searchParams.get("has");
        return json(has === "embed" ? facetCounts({ status: { warning: 1 }, has: { encode_video: 2, embed: 4 } }) : facetCounts({ status: { warning: 3 }, has: { encode_video: 5, embed: 6 } }));
      }
      if (path.startsWith("/files?")) return json(ROWS, 200, { next_cursor: null });
      return json([]);
    });
    const view = renderLibrary(api);
    const properties = await openFacet(view, "Properties");
    expect(await properties.findByRole("checkbox", { name: "Needs video encoding 5" })).toBeInTheDocument();

    await fireEvent.click(properties.getByRole("checkbox", { name: /^Subtitles embeddable/ }));
    expect(await properties.findByRole("checkbox", { name: "Needs video encoding 2" })).toBeInTheDocument();
    expect(properties.getByRole("checkbox", { name: "Subtitles embeddable 4" })).toBeChecked();
    const status = await openFacet(view, "Status");
    expect(status.getByRole("checkbox", { name: "Warning 1" })).toBeInTheDocument();
  });

  it("resets every filter and the search with Reset filters", async () => {
    const onFilter = vi.fn();
    const { api } = makeApi({ counts: COUNTS });
    const view = render(() => <LibraryPage api={api} filter={{ q: "Heat", status: ["blocker"], has: ["embed"] }} onOpen={open} onFilter={onFilter} />);
    expect(await view.findByRole("searchbox", { name: "Search files" })).toHaveValue("Heat");

    await fireEvent.click(view.getByRole("button", { name: "Reset filters" }));

    expect(onFilter).toHaveBeenLastCalledWith({ q: "", status: [], has: [] }, { replace: false });
    expect(view.getByRole("searchbox", { name: "Search files" })).toHaveValue("");
    await waitFor(() => expect(lastList()).not.toMatch(/status=|has=|q=/));
  });

  it("searches by path after a short pause", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });

    await fireEvent.input(view.getByRole("searchbox", { name: "Search files" }), { target: { value: "Heat" } });

    await waitFor(() => expect(lastList()).toContain("q=Heat"), { timeout: 2000 });
    expect(lastCounts()).toContain("q=Heat");
  });

  it("loads the next page on request and appends it", async () => {
    const { api } = makeApi({ rows: ROWS.slice(0, 2), next: "c2", more: ROWS.slice(2) });
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });

    await fireEvent.click(view.getByRole("button", { name: "Load more files" }));

    expect(await view.findByRole("link", { name: "Arrival (2016)" })).toBeInTheDocument();
    expect(view.getByRole("link", { name: "Heat" })).toBeInTheDocument();
    expect(api.request).toHaveBeenCalledWith(expect.stringContaining("cursor=c2"));
    expect(view.queryByRole("button", { name: "Load more files" })).not.toBeInTheDocument();
  });
});

describe("LibraryPage: the filter lives in the address", () => {
  it("starts on the filter it is given and asks the server for exactly that", async () => {
    const { api } = makeApi({ counts: COUNTS });
    const view = render(() => <LibraryPage api={api} filter={{ q: "", status: ["warning", "blocker"], has: ["embed"] }} onOpen={open} />);

    expect(await view.findByRole("button", { name: "Status: Warning, Blocker" })).toBeInTheDocument();
    expect(view.getByRole("button", { name: "Properties: Subtitles embeddable" })).toBeInTheDocument();
    await waitFor(() => expect(lastList()).toContain("status=warning%2Cblocker"));
    expect(lastList()).toContain("has=embed");
  });

  it("follows the filter when the address changes, as Back does", async () => {
    const { api } = makeApi({ counts: COUNTS });
    const [filter, setFilter] = createSignal({ q: "", status: ["blocker"], has: [] as string[] });
    const view = render(() => <LibraryPage api={api} filter={filter()} onOpen={open} />);
    await view.findByRole("button", { name: "Status: Blocker" });

    setFilter({ q: "Heat", status: [], has: ["embed"] });
    expect(await view.findByRole("button", { name: "Properties: Subtitles embeddable" })).toBeInTheDocument();
    expect(view.getByRole("button", { name: "Status" })).toBeInTheDocument();
    expect(view.getByRole("searchbox", { name: "Search files" })).toHaveValue("Heat");
    await waitFor(() => expect(lastList()).toContain("q=Heat"));
    expect(lastList()).not.toContain("status=");
  });

  it("reports a chosen value as a new address entry, and a search as a replacement", async () => {
    const onFilter = vi.fn();
    const { api } = makeApi({ counts: COUNTS });
    const view = render(() => <LibraryPage api={api} onOpen={open} onFilter={onFilter} />);
    const status = await openFacet(view, "Status");

    await fireEvent.click(await status.findByRole("checkbox", { name: "Warning 1" }));
    expect(onFilter).toHaveBeenLastCalledWith({ q: "", status: ["warning"], has: [] }, { replace: false });

    await fireEvent.input(view.getByRole("searchbox", { name: "Search files" }), { target: { value: " Heat " } });
    await waitFor(() => expect(onFilter).toHaveBeenLastCalledWith({ q: "Heat", status: ["warning"], has: [] }, { replace: true }), { timeout: 2000 });
  });
});

describe("LibraryPage: hardening", () => {
  it("keeps the rows on screen when loading more fails, and offers a retry", async () => {
    const { api } = makeApi({ rows: ROWS.slice(0, 2), next: "c2", more: ROWS.slice(2) });
    const base = api.request;
    let failNext = true;
    api.request = vi.fn(async (path: string, init?: RequestInit) => {
      if (path.includes("cursor=c2") && failNext) { failNext = false; return json({}, 503); }
      return base(path, init);
    });
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });

    await fireEvent.click(view.getByRole("button", { name: "Load more files" }));

    expect(await view.findByRole("alert")).toHaveTextContent("Unable to load more files");
    expect(view.getByRole("link", { name: "Heat" })).toBeInTheDocument();
    await fireEvent.click(view.getByRole("button", { name: "Retry loading more files" }));
    expect(await view.findByRole("link", { name: "Arrival (2016)" })).toBeInTheDocument();
    expect(view.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("treats a page without a data array as empty instead of crashing", async () => {
    const api: Api = { request: vi.fn(async (path: string) => (path.startsWith("/files?") ? new Response(JSON.stringify({ meta: {} })) : path === "/dashboard" ? json({ files: {}, jobs: {} }) : json([]))) };
    const view = renderLibrary(api);
    expect(await view.findByText("No files yet")).toBeInTheDocument();
  });
});

describe("LibraryPage: the re-encode state", () => {
  it("has a Needs re-encode box that asks the server for only those files", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });
    const properties = await openFacet(view, "Properties");

    await fireEvent.click(properties.getByRole("checkbox", { name: /Needs video encoding/ }));

    await waitFor(() => expect(lastList()).toContain("has=encode_video"));
    expect(properties.getByRole("checkbox", { name: /Needs video encoding/ })).toBeChecked();
    expect(properties.getByRole("checkbox", { name: /Converted, original kept/ })).not.toBeChecked();
  });

  it("previews a video that needs a re-encode and releases it only with the confirmed video level", async () => {
    const reencode = file({ id: "m_r1", display_path: "Film/Big (2020)/Big (2020).mp4", size: GB, compliance_state: "blocker", desired_action: "fix", encode_need: "video",
      compliance_reasons: ["[blocker] video-reencode: the video needs a re-encode"] });
    const { api, posts } = makeApi({ rows: [reencode] });
    const view = renderLibrary(api);
    await pickRow(view, "Big (2020)");

    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));
    const preview = view.getByRole("region", { name: "Prepare preview" });
    expect(posts()).toHaveLength(0);

    await fireEvent.click(within(preview).getByRole("radio", { name: /^Encode audio and video/ }));
    await fireEvent.click(within(preview).getByRole("checkbox", { name: /re-encoded/ }));
    await fireEvent.click(within(preview).getByRole("button", { name: "Prepare 1 file" }));
    await waitFor(() => expect(posts()).toHaveLength(1));
    expect(JSON.parse(String(posts()[0]!.init?.body))).toEqual({ action: "prepare", media_ids: ["m_r1"], manual_approval: true, encode: "all", confirm_video_encode: true });
  });
});

describe("LibraryPage: a mixed selection with a re-encode", () => {
  const reencode = file({ id: "m_r1", display_path: "Film/Big (2020)/Big (2020).mp4", size: GB, compliance_state: "blocker", desired_action: "fix", encode_need: "video",
    compliance_reasons: ["[blocker] video-reencode: the video needs a re-encode"] });
  const mkv = file({ id: "m_k1", display_path: "Film/Lynley (2001)/Lynley (2001).mkv", suffix: ".mkv", size: GB, compliance_state: "unknown", desired_action: "convert" });

  it("with Encode audio queues every file in one job, without a confirmation", async () => {
    const { api, posts } = makeApi({ rows: [reencode, mkv] });
    const view = renderLibrary(api);
    await pickRow(view, "Big (2020)");
    await pickRow(view, "Lynley (2001)");

    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));
    const preview = view.getByRole("region", { name: "Prepare preview" });
    expect(preview).toHaveTextContent("The original is kept.");
    expect(preview).toHaveTextContent("1 of 2 files need a re-encode");
    await fireEvent.click(within(preview).getByRole("button", { name: "Prepare 2 files" }));

    await waitFor(() => expect(posts()).toHaveLength(1));
    expect(JSON.parse(String(posts()[0]!.init?.body))).toEqual({ action: "prepare", media_ids: ["m_r1", "m_k1"], manual_approval: true, encode: "audio" });
  });

  it("with the confirmed video level queues every selected file in one job", async () => {
    const { api, posts } = makeApi({ rows: [reencode, mkv] });
    const view = renderLibrary(api);
    await pickRow(view, "Big (2020)");
    await pickRow(view, "Lynley (2001)");

    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));
    const preview = view.getByRole("region", { name: "Prepare preview" });
    await fireEvent.click(within(preview).getByRole("radio", { name: /^Encode audio and video/ }));
    await fireEvent.click(within(preview).getByRole("checkbox", { name: /re-encoded/ }));
    await fireEvent.click(within(preview).getByRole("button", { name: "Prepare 2 files" }));

    await waitFor(() => expect(posts()).toHaveLength(1));
    expect(JSON.parse(String(posts()[0]!.init?.body))).toEqual({ action: "prepare", media_ids: ["m_r1", "m_k1"], manual_approval: true, encode: "all", confirm_video_encode: true });
  });

  it("keeps the files in the preview when the queue is refused, and queues them once on a retry", async () => {
    const base = makeApi({ rows: [mkv] });
    let reject = true;
    const api: Api = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        if (path === "/jobs" && reject) return json({}, 503);
        return base.api.request(path, init);
      }),
    };
    const view = renderLibrary(api);
    await pickRow(view, "Lynley (2001)");
    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));

    await fireEvent.click(view.getByRole("button", { name: "Prepare 1 file" }));
    expect(await view.findByRole("alert")).toBeInTheDocument();
    const preview = await view.findByRole("region", { name: "Prepare preview" });
    expect(preview).toHaveTextContent("Lynley (2001)");

    reject = false;
    await fireEvent.click(within(preview).getByRole("button", { name: "Prepare 1 file" }));
    await view.findByText(/prepare job queued/i);

    expect(base.posts()).toHaveLength(1);
  });
});

describe("LibraryPage: selection and the fix flow", () => {
  const pick = async (view: ReturnType<typeof renderLibrary>, ...names: string[]) => {
    for (const name of names) await pickRow(view, name);
  };

  it("summarises the selection with scope before anything can run", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    await pick(view, "Pretty Woman (1990)", "Heat");

    const bar = view.getByRole("region", { name: "Selection" });
    expect(bar).toHaveTextContent("2 selected");
    expect(bar).toHaveTextContent("2 can be prepared");
    expect(bar).toHaveTextContent("18 GB to rewrite");
    expect(view.getByRole("button", { name: /preview and prepare/i })).toBeEnabled();
  });

  it("has no Fix All and no one-click Fix: a fix needs a selection and a preview", async () => {
    const { api, posts } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });

    expect(view.queryByRole("button", { name: "Fix All" })).not.toBeInTheDocument();
    expect(view.queryByRole("button", { name: "Prepare" })).not.toBeInTheDocument();
    expect(view.queryByRole("region", { name: "Selection" })).not.toBeInTheDocument();
    expect(posts()).toHaveLength(0);
  });

  it("previews the scope and queues the fix only after confirmation, with one stable idempotency key", async () => {
    const { api, posts } = makeApi();
    const view = renderLibrary(api);
    await pick(view, "Pretty Woman (1990)", "Heat");

    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));
    const preview = view.getByRole("region", { name: "Prepare preview" });
    expect(preview).toHaveTextContent("Pretty Woman (1990)");
    expect(preview).toHaveTextContent("Write 4 elng language tags");
    expect(preview).toHaveTextContent("Remove the dby1 brand from the file header");
    expect(preview).toHaveTextContent("18 GB");
    expect(preview).toHaveTextContent(/largest file/i);
    expect(posts()).toHaveLength(0);

    await fireEvent.click(view.getByRole("button", { name: "Prepare 2 files" }));

    expect(await view.findByText(/prepare job queued for 2 files/i)).toBeInTheDocument();
    expect(posts()).toHaveLength(1);
    const call = posts()[0]!;
    expect(call.init?.method).toBe("POST");
    expect(JSON.parse(String(call.init?.body))).toEqual({ action: "prepare", media_ids: ["m_f1", "m_f2"], manual_approval: true, encode: "audio" });
    expect(new Headers(call.init?.headers).get("Idempotency-Key")).toBeTruthy();
    expect(JSON.stringify((api.request as unknown as ReturnType<typeof vi.fn>).mock.calls)).not.toContain("convert");
    expect(view.queryByRole("region", { name: "Selection" })).not.toBeInTheDocument();
  });

  it("keeps the same idempotency key when a failed queue attempt is retried", async () => {
    let attempts = 0;
    const base = makeApi();
    const api: Api = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        if (path === "/jobs") return ++attempts === 1 ? json({}, 503) : json({ id: "j_1" }, 201);
        return base.api.request(path, init);
      }),
    };
    const view = renderLibrary(api);
    await pickRow(view, "Heat");
    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));

    await fireEvent.click(view.getByRole("button", { name: "Prepare 1 file" }));
    expect(await view.findByRole("alert")).toHaveTextContent(/unable to queue/i);
    await fireEvent.click(view.getByRole("button", { name: "Prepare 1 file" }));
    await view.findByText(/prepare job queued/i);

    const keys = (api.request as unknown as ReturnType<typeof vi.fn>).mock.calls
      .filter(([path]) => path === "/jobs")
      .map(([, init]) => new Headers((init as RequestInit).headers).get("Idempotency-Key"));
    expect(keys).toHaveLength(2);
    expect(keys[0]).toBeTruthy();
    expect(keys[1]).toBe(keys[0]);
  });

  it("leaves files that cannot be fixed out and says so", async () => {
    const { api, posts } = makeApi();
    const view = renderLibrary(api);
    await pick(view, "Heat", "Arrival (2016)");

    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));
    expect(view.getByRole("region", { name: "Prepare preview" })).toHaveTextContent(/1 selected file skipped/i);
    await fireEvent.click(view.getByRole("button", { name: "Prepare 1 file" }));
    await view.findByText(/prepare job queued for 1 file/i);

    expect(JSON.parse(String(posts()[0]!.init?.body))).toEqual({ action: "prepare", media_ids: ["m_f2"], manual_approval: true, encode: "audio" });
  });

  it("disables the preview when nothing selected can be fixed", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    await pickRow(view, "Arrival (2016)");

    expect(view.getByRole("button", { name: /preview and prepare/i })).toBeDisabled();
  });

  it("cancels the preview without queueing anything, by button or Escape", async () => {
    const { api, posts } = makeApi();
    const view = renderLibrary(api);
    await pickRow(view, "Heat");

    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));
    await fireEvent.click(view.getByRole("button", { name: "Cancel" }));
    expect(view.queryByRole("region", { name: "Prepare preview" })).not.toBeInTheDocument();

    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));
    await fireEvent.keyDown(view.getByRole("region", { name: "Prepare preview" }), { key: "Escape" });
    expect(view.queryByRole("region", { name: "Prepare preview" })).not.toBeInTheDocument();
    expect(posts()).toHaveLength(0);
  });

  it("queues a verification for the selection with the documented body", async () => {
    const { api, posts } = makeApi();
    const view = renderLibrary(api);
    await pick(view, "Whiplash (2014)", "Heat");

    await fireEvent.click(view.getByRole("button", { name: "Verify selection" }));

    expect(await view.findByText(/verification job queued/i)).toBeInTheDocument();
    expect(JSON.parse(String(posts()[0]!.init?.body))).toEqual({ action: "verify", media_ids: ["m_f2", "m_f4"], manual_approval: false });
  });

  it("clears the selection on request", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    await pickRow(view, "Heat");

    await fireEvent.click(view.getByRole("button", { name: "Clear selection" }));

    expect(view.queryByRole("region", { name: "Selection" })).not.toBeInTheDocument();
    expect(isPicked(view, "Heat")).toBe(false);
  });
});

describe("LibraryPage: scanning by hand", () => {
  it("offers a scan and reloads the list and the counts when it finishes", async () => {
    vi.useFakeTimers();
    let scanStatus = "running";
    const calls: string[] = [];
    const api: Api = {
      request: vi.fn(async (path: string) => {
        calls.push(path);
        if (path === "/roots") return json([{ id: "r_1", name: "Film", latest_scan: { id: "s_1", root_id: "r_1", generation: 1, status: scanStatus, started_at: null, finished_at: null } }]);
        if (path.startsWith("/files?")) return json(ROWS, 200, { next_cursor: null });
        if (path === "/dashboard") return json({ files: { clean: 1 }, scans: {}, jobs: {} });
        return json([]);
      }),
    };
    const view = renderLibrary(api);
    await vi.advanceTimersByTimeAsync(0);
    expect(view.getByRole("button", { name: "Scanning" })).toBeDisabled();
    const before = calls.filter((path) => path.startsWith("/files?")).length;

    scanStatus = "succeeded";
    await vi.advanceTimersByTimeAsync(4100);

    expect(view.getByRole("button", { name: "Scan library" })).toBeEnabled();
    expect(calls.filter((path) => path.startsWith("/files?")).length).toBe(before + 1);
    expect(calls.filter((path) => path === "/dashboard").length).toBe(2);
    vi.useRealTimers();
  });
});

describe("LibraryPage: a first run and a search that finds nothing", () => {
  const apiWith = (files: Record<string, number>, rows: unknown[] = []) => {
    const state = { stateParam: "" };
    const api: Api = {
      request: vi.fn(async (path: string) => {
        if (path === "/dashboard") return json({ files, scans: {}, jobs: {} });
        if (path === "/roots") return json([]);
        if (path.startsWith("/files?")) { state.stateParam = path; return json(rows, 200, { next_cursor: null }); }
        return json([]);
      }),
    };
    return { api, state };
  };

  it("teaches the next step when the library has never been scanned", async () => {
    const view = renderLibrary(apiWith({}).api);

    expect(await view.findByText("No files yet")).toBeInTheDocument();
    expect(view.getByText(/a scan reads your libraries/i)).toBeInTheDocument();
    expect(view.queryByRole("button", { name: "Reset filters" })).not.toBeInTheDocument();
  });

  it("offers to clear the filters when files exist but none match", async () => {
    const { api, state } = apiWith({ clean: 3 });
    const view = renderLibrary(api);
    const status = await openFacet(view, "Status");
    await fireEvent.click(status.getByRole("checkbox", { name: /^Blocker/ }));
    expect(await view.findByText(/no files match/i)).toBeInTheDocument();
    expect(state.stateParam).toContain("status=blocker");

    // One name for it everywhere: the empty list offers the same reset as the bar.
    const resets = view.getAllByRole("button", { name: "Reset filters" });
    expect(resets).toHaveLength(2);
    await fireEvent.click(resets[1]!);

    await waitFor(() => expect(state.stateParam).not.toContain("status="));
    expect(view.getByRole("button", { name: "Status" })).toBeInTheDocument();
  });
});

describe("LibraryPage: verifying from the keyboard", () => {
  it("V verifies the open file and the marked files, like the Verify buttons", async () => {
    const { api, posts } = makeApi();
    const view = renderLibrary(api);
    await pickRow(view, "Heat");
    await pickRow(view, "Whiplash (2014)");

    await fireEvent.keyDown(view.getByRole("link", { name: "Heat" }), { key: "v" });

    expect(await view.findByText(/verification job queued/i)).toBeInTheDocument();
    expect(JSON.parse(String(posts()[0]!.init?.body))).toEqual({ action: "verify", media_ids: ["m_f2", "m_f4"], manual_approval: false });
  });

  it("leaves V alone while typing in the search box", async () => {
    const { api, posts } = makeApi();
    const view = renderLibrary(api);
    await pickRow(view, "Heat");

    await fireEvent.keyDown(view.getByRole("searchbox", { name: "Search files" }), { key: "v" });

    expect(posts()).toHaveLength(0);
  });
});

describe("LibraryPage: acting on the open file when every response has its own ids", () => {
  /** The real server issues a fresh id for the same file in each response, so list, URL and detail ids differ. */
  function makeRotatingApi() {
    const calls: Array<{ path: string; init?: RequestInit }> = [];
    lastCalls = calls;
    const heat = ROWS[1]!;
    const api: Api = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        calls.push({ path, init });
        if (path === "/jobs") return json({ id: "j_1" }, 201);
        if (path.startsWith("/files?")) return json([heat], 200, { next_cursor: null });
        if (path.startsWith("/files/") && !path.includes("/", 7)) {
          return json({ ...heat, id: "m_from_detail", status_version: 1, last_success_at: null, has_error: false, sidecar_count: 0, inspection_available: true });
        }
        return json([]);
      }),
    };
    return { api, posts: () => calls.filter((call) => call.path === "/jobs") };
  }

  it("verifies the open file even though the detail id differs from the list and address ids", async () => {
    const { api, posts } = makeRotatingApi();
    const view = renderLibrary(api, "m_from_address");

    await fireEvent.click(await view.findByRole("button", { name: "Verify" }));

    await waitFor(() => expect(posts()).toHaveLength(1));
    expect(JSON.parse(String(posts()[0]!.init?.body))).toEqual({ action: "verify", media_ids: ["m_from_detail"], manual_approval: false });
    expect(await view.findByText(/verification job queued/i)).toBeInTheDocument();
  });

  it("opens the fix preview for the open file and queues the fix for it", async () => {
    const { api, posts } = makeRotatingApi();
    const view = renderLibrary(api, "m_from_address");

    await fireEvent.click(await view.findByRole("button", { name: /prepare preview/i }));
    const preview = await view.findByRole("region", { name: "Prepare preview" });
    await fireEvent.click(within(preview).getByRole("button", { name: /^(prepare|confirm)/i }));

    await waitFor(() => expect(posts()).toHaveLength(1));
    expect(JSON.parse(String(posts()[0]!.init?.body))).toEqual({ action: "prepare", media_ids: ["m_from_detail"], manual_approval: true, encode: "audio" });
  });
});

describe("LibraryPage: following jobs without a reload", () => {
  /** The server issues fresh ids on every response and reports job counts on the dashboard. */
  function makeLiveApi(state: { status: string; jobs: Record<string, number>; reasons?: string[] }) {
    let serial = 0;
    const calls: string[] = [];
    const heat = ROWS[1]!;
    const api: Api = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        calls.push(path);
        serial += 1;
        if (path === "/dashboard") return json({ files: { [state.status]: 1 }, scans: {}, jobs: state.jobs });
        if (path === "/jobs" && init?.method === "POST") { state.jobs = { queued: 1 }; return json({ id: "j_1" }, 201); }
        const current = { ...heat, id: `m_${serial}`, compliance_state: state.status, compliance_reasons: state.reasons ?? [] };
        if (path.startsWith("/files?")) return json([current], 200, { next_cursor: null });
        if (path.startsWith("/files/") && !path.includes("/", 7)) return json({ ...current, status_version: 1, last_success_at: null, has_error: false, sidecar_count: 0, inspection_available: true });
        return json([]);
      }),
    };
    return { api, calls, filesCalls: () => calls.filter((path) => path.startsWith("/files?")).length };
  }

  it("updates the status in the table while a job runs and stops asking once nothing is active", async () => {
    vi.useFakeTimers();
    const state = { status: "warning", jobs: { running: 1 } as Record<string, number> };
    const live = makeLiveApi(state);
    const view = renderLibrary(live.api);
    await vi.advanceTimersByTimeAsync(0);
    expect(within(view.container.querySelector("tbody") as HTMLElement).getByText("Warning")).toBeInTheDocument();

    state.status = "clean";
    state.jobs = {};
    await vi.advanceTimersByTimeAsync(4100);

    expect(within(view.container.querySelector("tbody") as HTMLElement).getByText("Ready")).toBeInTheDocument();
    const settled = live.filesCalls();
    await vi.advanceTimersByTimeAsync(12000);
    expect(live.filesCalls()).toBe(settled);
    vi.useRealTimers();
  });

  it("rebuilds only the rows that changed", async () => {
    vi.useFakeTimers();
    let serial = 0;
    let heatState = "warning";
    const state = { jobs: { running: 1 } as Record<string, number> };
    const api: Api = {
      request: vi.fn(async (path: string) => {
        serial += 1;
        if (path === "/dashboard") return json({ files: {}, scans: {}, jobs: state.jobs });
        if (path.startsWith("/files?")) {
          return json([
            { ...ROWS[0]!, id: `m_p${serial}` },
            { ...ROWS[1]!, id: `m_h${serial}`, compliance_state: heatState },
          ], 200, { next_cursor: null });
        }
        return json([]);
      }),
    };
    const view = renderLibrary(api);
    await vi.advanceTimersByTimeAsync(0);
    const rowOf = (name: string) => rowNamed(view, name);
    const prettyWoman = rowOf("Pretty Woman (1990)");

    heatState = "clean";
    await vi.advanceTimersByTimeAsync(4100);

    expect(rowOf("Pretty Woman (1990)")).toBe(prettyWoman);
    expect(within(rowOf("Heat")).getByText("Ready")).toBeInTheDocument();
    expect(prettyWoman.isConnected).toBe(true);
    // The row whose status changed is marked for a moment; the one that did not is left alone.
    expect(rowOf("Heat")).toHaveClass("flash");
    expect(rowOf("Pretty Woman (1990)")).not.toHaveClass("flash");
    await vi.advanceTimersByTimeAsync(900);
    expect(rowOf("Heat")).not.toHaveClass("flash");
    vi.useRealTimers();
  });

  it("keeps the selection and the open file across a refresh that renames every id", async () => {
    vi.useFakeTimers();
    const state = { status: "warning", jobs: { running: 1 } as Record<string, number> };
    const live = makeLiveApi(state);
    const view = renderLibrary(live.api);
    await vi.advanceTimersByTimeAsync(0);
    await pickRow(view, "Heat");
    expect(view.getByRole("region", { name: "Selection" })).toHaveTextContent("1 selected");

    await vi.advanceTimersByTimeAsync(4100);

    expect(isPicked(view, "Heat")).toBe(true);
    expect(view.getByRole("region", { name: "Selection" })).toHaveTextContent("1 selected");
    vi.useRealTimers();
  });

  it("brings the open file's detail up to date as well", async () => {
    vi.useFakeTimers();
    const state = { status: "warning", jobs: { running: 1 } as Record<string, number>, reasons: ["[warning] apple-tags: 4 tracks lack an elng tag"] };
    const live = makeLiveApi(state);
    const view = renderLibrary(live.api, "m_open");
    await vi.advanceTimersByTimeAsync(0);
    const detail = view.container.querySelector(".insp-verdict") as HTMLElement;
    expect(detail).toHaveTextContent("Warning");

    state.status = "clean";
    state.reasons = ["No compliance violations reported by verify."];
    state.jobs = {};
    await vi.advanceTimersByTimeAsync(4100);

    expect(view.container.querySelector(".insp-verdict")).toHaveTextContent("Ready");
    vi.useRealTimers();
  });

  it("starts following as soon as a verification is queued, even when nothing was running before", async () => {
    vi.useFakeTimers();
    const state = { status: "warning", jobs: {} as Record<string, number> };
    const live = makeLiveApi(state);
    const view = renderLibrary(live.api);
    await vi.advanceTimersByTimeAsync(0);
    const before = live.filesCalls();
    await vi.advanceTimersByTimeAsync(9000);
    expect(live.filesCalls()).toBe(before);

    await pickRow(view, "Heat");
    await fireEvent.click(view.getByRole("button", { name: "Verify selection" }));
    state.status = "clean";
    state.jobs = {};
    await vi.advanceTimersByTimeAsync(4100);

    expect(within(view.container.querySelector("tbody") as HTMLElement).getByText("Ready")).toBeInTheDocument();
    vi.useRealTimers();
  });
});

describe("LibraryPage: a file whose fix is already in progress", () => {
  const withActiveFix = ROWS.map((row) => (row.id === "m_f2" ? { ...row, fix_status: "queued" } : row));

  it("cannot be selected for another fix", async () => {
    const { api } = makeApi({ rows: withActiveFix });
    const view = renderLibrary(api);
    await pickRow(view, "Heat");
    await pickRow(view, "Pretty Woman (1990)");

    const bar = view.getByRole("region", { name: "Selection" });

    expect(bar).toHaveTextContent("2 selected");
    expect(bar).toHaveTextContent("1 can be prepared");
  });

  it("shows the fix as queued right after it was queued, without waiting for a reload", async () => {
    const heat = ROWS[1]!;
    let fixStatus: string | null = null;
    const calls: Array<{ path: string; init?: RequestInit }> = [];
    const api: Api = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        calls.push({ path, init });
        if (path === "/jobs" && init?.method === "POST") { fixStatus = "queued"; return json({ id: "j_1" }, 201); }
        if (path.startsWith("/files?")) return json([{ ...heat, fix_status: fixStatus }], 200, { next_cursor: null });
        if (path.startsWith("/files/") && !path.includes("/", 7)) return json({ ...heat, id: "m_from_detail", fix_status: fixStatus, status_version: 1, last_success_at: null, has_error: false, sidecar_count: 0, inspection_available: true });
        return json([]);
      }),
    };
    const view = renderLibrary(api, "m_from_address");

    await fireEvent.click(await view.findByRole("button", { name: /prepare preview/i }));
    const preview = await view.findByRole("region", { name: "Prepare preview" });
    await fireEvent.click(within(preview).getByRole("button", { name: /^Prepare/ }));

    await waitFor(() => expect(calls.filter((call) => call.path === "/jobs")).toHaveLength(1));
    expect(await view.findByRole("button", { name: "Prepare queued" })).toBeDisabled();
  });
});

describe("LibraryPage: what a fix would really change", () => {
  const levelOnly = file({
    id: "m_f5", display_path: "Film/Tenet (2020)/Tenet (2020).mp4", size: 28 * GB, compliance_state: "warning", desired_action: "fix",
    compliance_reasons: ["[warning] avc-level-atv-limit: H.264 level 6.0 exceeds 5.2"],
  });
  const mixed = file({
    id: "m_f6", display_path: "Film/Dune (2024)/Dune (2024).mp4", size: 24 * GB, compliance_state: "warning", desired_action: "fix",
    compliance_reasons: ["[warning] fast-start: moov atom appears after mdat", "[warning] audio-sample-rate: 96 kHz"],
  });

  it("does not offer a fix for a file whose only problem a fix cannot change", async () => {
    const { api } = makeApi({ rows: [levelOnly] });
    const view = renderLibrary(api);
    await pickRow(view, "Tenet (2020)");

    expect(view.getByRole("region", { name: "Selection" })).toHaveTextContent("0 can be prepared");
    expect(view.getByRole("button", { name: /preview and prepare/i })).toBeDisabled();
  });

  it("separates what a fix changes from what stays, and says when a remux is involved", async () => {
    const { api } = makeApi({ rows: [mixed] });
    const view = renderLibrary(api);
    await pickRow(view, "Dune (2024)");

    await fireEvent.click(view.getByRole("button", { name: /preview and prepare/i }));
    const preview = view.getByRole("region", { name: "Prepare preview" });

    expect(preview).toHaveTextContent("Rewrite the file with the index at the front (faststart)");
    expect(within(preview).getByText("Remux")).toBeInTheDocument();
    expect(within(preview).getByText("Unchanged")).toBeInTheDocument();
    expect(preview).toHaveTextContent(/needs a re-encode/i);
    expect(preview).toHaveTextContent(/1 of 2 findings? resolved/i);
  });
});

describe("LibraryPage: keyboard", () => {
  it("moves through the list with the arrow keys, selecting as it goes, and toggles with Space", async () => {
    open.mockClear();
    const { api } = makeApi();
    const [active, setActive] = createSignal<string | undefined>("m_f1");
    const view = render(() => <LibraryPage api={api} activeId={active()} onOpen={(id) => { open(id); setActive(id); }} />);
    const row = (await view.findByRole("link", { name: "Pretty Woman (1990)" })).closest("tr") as HTMLElement;
    row.focus();

    await fireEvent.keyDown(row, { key: "ArrowDown" });
    expect(open).toHaveBeenLastCalledWith("m_f2");
    const second = (await view.findByRole("link", { name: "Heat" })).closest("tr") as HTMLElement;
    // Moving selects the row, so its box is ticked; Space takes it out of the selection again.
    expect(isPicked(view, "Heat")).toBe(true);
    await fireEvent.keyDown(second, { key: " " });
    expect(isPicked(view, "Heat")).toBe(false);
    await fireEvent.keyDown(second, { key: "ArrowUp" });
    expect(open).toHaveBeenLastCalledWith("m_f1");
  });

  it("opens the fix preview for the active file with F", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api, "m_f1");
    const row = (await view.findByRole("link", { name: "Pretty Woman (1990)" })).closest("tr") as HTMLElement;

    await fireEvent.keyDown(row, { key: "f" });

    expect(await view.findByRole("region", { name: "Prepare preview" })).toHaveTextContent("Pretty Woman (1990)");
  });

  it("does not hijack typing in the search box", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api, "m_f1");
    const search = await view.findByRole("searchbox", { name: "Search files" });

    await fireEvent.keyDown(search, { key: "f" });

    expect(view.queryByRole("region", { name: "Prepare preview" })).not.toBeInTheDocument();
  });
});

describe("LibraryPage: removing missing files", () => {
  it("offers the button only while files are missing, asks first, then removes and reloads", async () => {
    let missing = 2;
    const posts: Array<{ path: string; init?: RequestInit }> = [];
    const api: Api = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        if (path === "/dashboard") return json({ files: { clean: 1 }, missing, scans: {}, jobs: {} });
        if (path === "/files/dismiss-missing") { posts.push({ path, init }); missing = 0; return json({ removed: 2 }); }
        if (path.startsWith("/files?")) return json(ROWS, 200, { next_cursor: null });
        return json([]);
      }),
    };
    const view = renderLibrary(api);

    await fireEvent.click(await view.findByRole("button", { name: "Remove missing (2)" }));
    expect(posts).toHaveLength(0);
    const confirm = await view.findByRole("region", { name: "Remove missing files" });
    expect(confirm).toHaveTextContent("Nothing is deleted from disk");

    await fireEvent.click(within(confirm).getByRole("button", { name: "Confirm remove" }));

    await waitFor(() => expect(posts).toHaveLength(1));
    expect(posts[0]?.init?.method).toBe("POST");
    expect(await view.findByRole("status")).toHaveTextContent("Removed 2 missing files");
    await waitFor(() => expect(view.queryByRole("button", { name: /Remove missing/ })).not.toBeInTheDocument());
  });

  it("keeps the question open and says so when removing fails", async () => {
    const api: Api = {
      request: vi.fn(async (path: string) => {
        if (path === "/dashboard") return json({ files: {}, missing: 1, scans: {}, jobs: {} });
        if (path === "/files/dismiss-missing") return json({}, 500);
        if (path.startsWith("/files?")) return json(ROWS, 200, { next_cursor: null });
        return json([]);
      }),
    };
    const view = renderLibrary(api);

    await fireEvent.click(await view.findByRole("button", { name: "Remove missing (1)" }));
    await fireEvent.click(await view.findByRole("button", { name: "Confirm remove" }));

    expect(await view.findByRole("alert")).toHaveTextContent("Unable to remove missing files");
    expect(view.getByRole("region", { name: "Remove missing files" })).toBeInTheDocument();
  });
});

describe("LibraryPage: files that are gone", () => {
  it("says so in the reason column instead of repeating an old verify finding", async () => {
    const gone = file({ id: "m_gone", display_path: "Film/Gone (2001)/Gone.mp4", size: GB, compliance_state: "blocker", discovery_state: "missing",
      compliance_reasons: ["[blocker] ftyp-dby1-requires-atmos: ftyp lists dby1 but no track is Atmos"] });
    const { api } = makeApi({ rows: [gone] });
    const view = renderLibrary(api);

    const row = (await view.findByRole("link", { name: "Gone" })).closest("tr") as HTMLElement;
    expect(row).toHaveTextContent("Missing");
    expect(row).toHaveTextContent("No longer on disk");
    expect(row).not.toHaveTextContent("ftyp lists dby1");
  });
});

describe("LibraryPage: list and columns", () => {
  it("switches to columns, remembers the choice, and lets a search show its hits as a list", async () => {
    localStorage.removeItem("atvprep.library.view");
    const { api, calls } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });

    await fireEvent.click(view.getByRole("button", { name: /Columns/ }));

    expect(await view.findByRole("group", { name: /Files by folder/ })).toBeInTheDocument();
    expect(view.getByRole("button", { name: /Columns/ })).toHaveAttribute("aria-pressed", "true");
    expect(calls.some((call) => call.path.startsWith("/browse"))).toBe(true);
    expect(localStorage.getItem("atvprep.library.view")).toBe("columns");

    await fireEvent.input(view.getByRole("searchbox", { name: "Search files" }), { target: { value: "heat" } });
    await waitFor(() => expect(view.queryByRole("group", { name: /Files by folder/ })).not.toBeInTheDocument());
    expect(await view.findByRole("table", { name: "Media files" })).toBeVisible();
    localStorage.removeItem("atvprep.library.view");
  });
});

describe("LibraryPage: actions on a column selection", () => {
  const mp4 = (name: string, over: Record<string, unknown> = {}) => file({ id: `m_${name}`, display_path: `Show/S01/${name}`, size: GB, compliance_state: "warning", desired_action: "fix", compliance_reasons: ["[warning] apple-tags: 4 tracks lack an elng tag"], ...over });
  const level = (folders: unknown[], files: unknown[] = []) => ({ folders, files, truncated: false });
  function columnsApi() {
    const posts: Array<{ path: string; body: unknown }> = [];
    const api: Api = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        if (init?.method === "POST") { posts.push({ path, body: init.body ? JSON.parse(String(init.body)) : null }); return json({ id: "j_1" }, 201); }
        if (path === "/dashboard") return json({ files: {}, missing: 0, scans: {}, jobs: {} });
        if (path.startsWith("/files?")) return json([], 200, { next_cursor: null });
        const query = new URLSearchParams(path.split("?")[1] ?? "");
        if (path.startsWith("/browse/below")) return json({ files: [mp4("e1.mp4"), mp4("e2.mp4"), mp4("sub.mkv", { suffix: ".mkv" })], truncated: false });
        if (path.startsWith("/browse")) {
          if (!query.has("root_id")) return json(level([{ name: "Film", id: "r_1", counts: { warning: 3 }, total: 3 }]));
          if (!query.get("path")) return json(level([{ name: "Show", counts: { warning: 3 }, total: 3 }]));
          if (query.get("path") === "Show") return json(level([{ name: "S01", counts: { warning: 2 }, total: 2 }, { name: "S02", counts: { warning: 1 }, total: 1 }]));
          return json(level([], [mp4("e1.mp4"), mp4("e2.mp4")]));
        }
        return json([]);
      }),
    };
    return { api, posts };
  }
  const row = (view: ReturnType<typeof renderLibrary>, name: string) =>
    Array.from(view.container.querySelectorAll<HTMLElement>("[role=option]")).find((el) => el.querySelector(".col-name")?.textContent === name) as HTMLElement;

  async function inShow(view: ReturnType<typeof renderLibrary>) {
    await fireEvent.click(await view.findByText("Film"));
    await fireEvent.click(await view.findByText("Show"));
    await view.findByText("S02");
  }

  it("verifies every MP4 below two selected folders and leaves out what is not MP4", async () => {
    localStorage.setItem("atvprep.library.view", "columns");
    const { api, posts } = columnsApi();
    const view = renderLibrary(api);
    await inShow(view);

    await fireEvent.click(row(view, "S01"));
    await fireEvent.click(row(view, "S02"), { metaKey: true });
    const bar = await view.findByRole("region", { name: "Selection" });
    expect(bar).toHaveTextContent("2 selected");
    expect(bar).toHaveTextContent("2 folders, with everything below");
    await fireEvent.click(within(bar).getByRole("button", { name: /Verify/ }));

    await waitFor(() => expect(posts.filter((post) => post.path === "/jobs")).toHaveLength(1));
    const job = posts.find((post) => post.path === "/jobs")?.body as { action: string; media_ids: string[] };
    expect(job.action).toBe("verify");
    expect(job.media_ids).toEqual(["m_e1.mp4", "m_e2.mp4"]);
    expect(await view.findByText(/Verification queued for 2 files; 1 skipped because they are not MP4/)).toBeInTheDocument();
    localStorage.removeItem("atvprep.library.view");
  });

  it("opens the fix preview for a selection from the right-click menu", async () => {
    localStorage.setItem("atvprep.library.view", "columns");
    const { api } = columnsApi();
    const view = renderLibrary(api);
    await inShow(view);

    await fireEvent.click(row(view, "S01"));
    await fireEvent.contextMenu(row(view, "S01"));
    await fireEvent.click(within(document.body).getByRole("menuitem", { name: /Prepare preview/ }));

    expect(await view.findByRole("region", { name: "Prepare preview" })).toBeInTheDocument();
    localStorage.removeItem("atvprep.library.view");
  });

  it("queues one inspection job for the selected files", async () => {
    localStorage.setItem("atvprep.library.view", "columns");
    const { api, posts } = columnsApi();
    const view = renderLibrary(api);
    await inShow(view);
    await fireEvent.click(row(view, "S01"));
    await fireEvent.click(await view.findByText("e1.mp4"));
    await fireEvent.click(row(view, "e2.mp4"), { metaKey: true });

    await fireEvent.contextMenu(row(view, "e2.mp4"));
    await fireEvent.click(within(document.body).getByRole("menuitem", { name: /Re-inspect 2 items/ }));

    await waitFor(() => expect(posts.filter((post) => post.path === "/jobs")).toHaveLength(1));
    const job = posts.find((post) => post.path === "/jobs")!.body as { action: string; media_ids: string[] };
    expect(job).toMatchObject({ action: "inspect", manual_approval: false });
    expect(job.media_ids).toHaveLength(2);
    expect(await view.findByText(/Inspection queued for 2 files/)).toBeInTheDocument();
    localStorage.removeItem("atvprep.library.view");
  });
});

describe("LibraryPage: selecting in the list like the Finder", () => {
  const rowOf = (view: ReturnType<typeof renderLibrary>, name: string) => view.getByRole("link", { name }).closest("tr") as HTMLElement;
  const checked = (view: ReturnType<typeof renderLibrary>) =>
    Array.from(view.container.querySelectorAll<HTMLElement>("tbody tr[data-picked=true]")).map((row) => `Select ${row.querySelector(".file-title")?.textContent}`);

  it("selects one row on a click, adds with Cmd or Ctrl, and takes a range with Shift", async () => {
    open.mockClear();
    const { api } = makeApi();
    const [active, setActive] = createSignal<string | undefined>(undefined);
    const view = render(() => <LibraryPage api={api} activeId={active()} onOpen={(id) => { open(id); setActive(id); }} />);
    await view.findByRole("link", { name: "Heat" });

    await fireEvent.click(rowOf(view, "Pretty Woman (1990)"));
    expect(open).toHaveBeenLastCalledWith("m_f1");
    expect(checked(view)).toEqual(["Select Pretty Woman (1990)"]);

    await fireEvent.click(rowOf(view, "Arrival (2016)"), { metaKey: true });
    expect(checked(view)).toEqual(["Select Pretty Woman (1990)", "Select Arrival (2016)"]);
    // Two files: the inspector has nothing single to show.
    expect(open).toHaveBeenLastCalledWith(undefined);
    expect(await view.findByRole("region", { name: "Selection" })).toHaveTextContent("2 selected");

    await fireEvent.click(rowOf(view, "Pretty Woman (1990)"));
    await fireEvent.click(rowOf(view, "Arrival (2016)"), { shiftKey: true });
    expect(checked(view)).toEqual(["Select Pretty Woman (1990)", "Select Heat", "Select Arrival (2016)"]);

    await fireEvent.click(rowOf(view, "Heat"), { ctrlKey: true });
    // Ctrl is the add key everywhere but on a Mac.
    expect(checked(view).length).toBeGreaterThanOrEqual(2);
  });

  it("does not show a selection bar for the one file that is merely open", async () => {
    const { api } = makeApi();
    const [active, setActive] = createSignal<string | undefined>(undefined);
    const view = render(() => <LibraryPage api={api} activeId={active()} onOpen={(id) => setActive(id)} />);
    await view.findByRole("link", { name: "Heat" });

    await fireEvent.click(rowOf(view, "Heat"));
    expect(view.queryByRole("region", { name: "Selection" })).not.toBeInTheDocument();

    await fireEvent.click(rowOf(view, "Arrival (2016)"), { metaKey: true });
    expect(await view.findByRole("region", { name: "Selection" })).toHaveTextContent("2 selected");
  });

  it("selects every row with Cmd+A and lets go with Escape", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    const first = rowOf(view, await view.findByRole("link", { name: "Heat" }).then(() => "Heat"));

    await fireEvent.keyDown(first, { key: "a", metaKey: true });
    expect(checked(view)).toHaveLength(4);

    await fireEvent.keyDown(first, { key: "Escape" });
    expect(checked(view)).toHaveLength(0);
  });

  it("extends the selection with Shift and the arrow keys", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });

    await fireEvent.click(rowOf(view, "Pretty Woman (1990)"));
    await fireEvent.keyDown(rowOf(view, "Pretty Woman (1990)"), { key: "ArrowDown", shiftKey: true });
    await fireEvent.keyDown(rowOf(view, "Heat"), { key: "ArrowDown", shiftKey: true });

    expect(checked(view)).toEqual(["Select Pretty Woman (1990)", "Select Heat", "Select Arrival (2016)"]);
  });

  it("opens a context menu for the selection and verifies it from there", async () => {
    const { api, posts } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });
    await fireEvent.click(rowOf(view, "Pretty Woman (1990)"));
    await fireEvent.click(rowOf(view, "Heat"), { metaKey: true });

    await fireEvent.contextMenu(rowOf(view, "Heat"), { clientX: 200, clientY: 200 });
    const menu = within(document.body).getByRole("menu", { name: "Actions for the selection" });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "Verify 2 filesV", "Prepare preview…F", "Re-inspect 2 filesI", "Copy names", "Copy paths", expect.stringMatching(/^Select all/),
    ]);
    await fireEvent.click(within(menu).getByRole("menuitem", { name: /Verify 2 files/ }));

    await waitFor(() => expect(posts()).toHaveLength(1));
    expect(JSON.parse(String(posts()[0]?.init?.body))).toEqual({ action: "verify", media_ids: ["m_f1", "m_f2"], manual_approval: false });
  });

  it("acts on the row under a right click when it is outside the selection", async () => {
    open.mockClear();
    const { api } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });
    await fireEvent.click(rowOf(view, "Pretty Woman (1990)"));

    await fireEvent.contextMenu(rowOf(view, "Arrival (2016)"));

    expect(checked(view)).toEqual(["Select Arrival (2016)"]);
    expect(within(document.body).getByRole("menuitem", { name: /Verify file/ })).toBeInTheDocument();
    await fireEvent.keyDown(within(document.body).getByRole("menu"), { key: "Escape" });
  });

  it("opens the fix preview from the menu", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });
    await fireEvent.click(rowOf(view, "Pretty Woman (1990)"));
    await fireEvent.contextMenu(rowOf(view, "Pretty Woman (1990)"));

    await fireEvent.click(within(document.body).getByRole("menuitem", { name: /Prepare preview/ }));

    expect(await view.findByRole("region", { name: "Prepare preview" })).toBeInTheDocument();
  });
});

describe("LibraryPage: the list has no tick boxes", () => {
  it("selects by clicking the row and says so for assistive technology, without a checkbox column", async () => {
    const { api } = makeApi();
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "Heat" });

    expect(view.container.querySelector("tbody input[type=checkbox]")).toBeNull();
    expect(within(view.getByRole("table", { name: "Media files" })).queryByRole("checkbox")).not.toBeInTheDocument();
    expect(view.getByRole("table", { name: "Media files" }).querySelectorAll("thead th")).toHaveLength(4);

    await pickRow(view, "Heat");
    await pickRow(view, "Arrival (2016)");

    expect(rowNamed(view, "Heat")).toHaveAttribute("aria-selected", "true");
    expect(rowNamed(view, "Arrival (2016)")).toHaveAttribute("aria-selected", "true");
    expect(rowNamed(view, "Pretty Woman (1990)")).toHaveAttribute("aria-selected", "false");
  });
});


describe("LibraryPage: re-inspecting several files, like Verify", () => {
  const jobs = (calls: Array<{ path: string; init?: RequestInit }>) =>
    calls.filter((call) => call.path === "/jobs").map((call) => JSON.parse(String(call.init?.body)) as { action: string; media_ids: string[]; manual_approval: boolean });

  it("has a Re-inspect button in the selection bar that queues ONE job for all picked files", async () => {
    const { api, calls } = makeApi();
    const view = renderLibrary(api);
    await pickRow(view, "Heat");
    await pickRow(view, "Arrival (2016)");

    await fireEvent.click(view.getByRole("button", { name: /Re-inspect selection/ }));

    await waitFor(() => expect(jobs(calls)).toHaveLength(1));
    expect(jobs(calls)[0]).toMatchObject({ action: "inspect", manual_approval: false });
    expect(jobs(calls)[0]!.media_ids).toEqual(expect.arrayContaining(["m_f2", "m_f3"]));
    expect(jobs(calls)[0]!.media_ids).toHaveLength(2);
    expect(await view.findByText(/Inspection queued for 2 files/)).toBeInTheDocument();
    expect(calls.some((call) => call.path.endsWith("/inspect"))).toBe(false);
  });

  it("does the same on the I key, for the selection or else the open file", async () => {
    const { api, calls } = makeApi();
    const view = renderLibrary(api);
    await pickRow(view, "Heat");
    await pickRow(view, "Whiplash (2014)");

    await fireEvent.keyDown(rowNamed(view, "Heat"), { key: "i" });

    await waitFor(() => expect(jobs(calls)).toHaveLength(1));
    expect(jobs(calls)[0]!.media_ids).toHaveLength(2);
  });

  it("leaves files that are missing out, and says so when nothing is left", async () => {
    const gone = file({ id: "m_g1", display_path: "Film/Gone (2001)/Gone (2001).mp4", size: GB, compliance_state: "clean", discovery_state: "missing" });
    const { api, calls } = makeApi({ rows: [gone] });
    const view = renderLibrary(api);
    await pickRow(view, "Gone (2001)");

    await fireEvent.click(view.getByRole("button", { name: /Re-inspect selection/ }));

    expect(await view.findByRole("alert")).toHaveTextContent(/files are missing/);
    expect(jobs(calls)).toHaveLength(0);
  });

  it("splits more than 100 files into jobs of 100, because that is what the server takes", async () => {
    const many = Array.from({ length: 230 }, (_, i) => file({ id: `m_${i}`, display_path: `Film/F${i}/F${i}.mp4`, size: GB, compliance_state: "clean" }));
    const { api, calls } = makeApi({ rows: many });
    const view = renderLibrary(api);
    await view.findByRole("link", { name: "F0" });
    await fireEvent.keyDown(view.getByRole("table", { name: "Media files" }), { key: "a", metaKey: true });
    await fireEvent.click(await view.findByRole("button", { name: /Re-inspect selection/ }));

    await waitFor(() => expect(jobs(calls).map((job) => job.media_ids.length)).toEqual([100, 100, 30]));
    expect(await view.findByText(/Inspection queued for 230 files/)).toBeInTheDocument();
  });
});

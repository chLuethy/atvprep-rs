import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { ColumnsView } from "../src/features/files/ColumnsView";
import { allReady, describeCounts, worstIn } from "../src/features/files/browse";
import type { FileRow } from "../src/features/files/loaders";
import { EMPTY_FILTER } from "../src/features/files/filters";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

const file = (name: string, over: Partial<FileRow> = {}): FileRow => ({
  id: `m_${name}`, root_id: "r_1", display_path: `deutsch/Film A (2020)/${name}`, kind: "movie", suffix: ".mp4", size: 1, mtime_ns: 1,
  discovery_state: "present", compliance_state: "clean", workflow_state: "idle", desired_action: "none", last_seen_at: null, ...over,
});

const LEVELS: Record<string, unknown> = {
  "": { folders: [{ name: "Film", id: "r_1", counts: { blocker: 1, warning: 2, clean: 3 }, total: 6 }, { name: "TV", id: "r_2", counts: { clean: 2 }, total: 2 }], files: [] },
  "r_1|": { folders: [{ name: "deutsch", counts: { blocker: 1, warning: 2, clean: 3 }, total: 6 }], files: [] },
  "r_1|deutsch": { folders: [{ name: "Film A (2020)", counts: { blocker: 1, clean: 1 }, total: 2 }, { name: "Film B (2021)", counts: { clean: 4 }, total: 4 }], files: [] },
  "r_1|deutsch/Film A (2020)": { folders: [], files: [file("A.mp4", { compliance_state: "blocker" }), file("A 2.mp4")] },
};

function makeApi() {
  const calls: string[] = [];
  const api: Api = {
    request: vi.fn(async (path: string) => {
      calls.push(path);
      const query = new URLSearchParams(path.split("?")[1] ?? "");
      const key = query.has("root_id") ? `${query.get("root_id")}|${query.get("path") ?? ""}` : "";
      const level = LEVELS[key];
      return level ? new Response(JSON.stringify({ data: level, meta: {} })) : new Response("{}", { status: 404 });
    }),
  };
  return { api, calls };
}

/** The row of a column by its name; a folder also appears in the path bar, so text alone is ambiguous. */
const option = (view: ReturnType<typeof render>, name: string) =>
  Array.from(view.container.querySelectorAll<HTMLElement>("[role=option]")).find((row) => row.querySelector(".col-name")?.textContent === name);

const show = (api: Api, over: Partial<Parameters<typeof ColumnsView>[0]> = {}) => {
  const onOpen = vi.fn();
  const view = render(() => <ColumnsView api={api} version={0} onOpen={onOpen} {...over} />);
  return { view, onOpen };
};

describe("browse helpers", () => {
  it("shows the worst state of a folder and describes all of it", () => {
    expect(worstIn({ clean: 5 })).toBeNull();
    expect(worstIn({ clean: 5, unknown: 2 })).toMatchObject({ kind: "neutral", count: 2 });
    expect(worstIn({ warning: 3, unknown: 2 })).toMatchObject({ kind: "warn", count: 3 });
    expect(worstIn({ warning: 3, blocker: 1, probe_failed: 1 })).toMatchObject({ kind: "bad", count: 2, label: "blockers" });
    expect(describeCounts({ blocker: 1, warning: 2, clean: 30, unknown: 3 })).toBe("1 blocker, 2 warnings, 3 not verified, 30 ready");
    expect(describeCounts({ clean: 4, embed_open: 2 })).toBe("2 embedding open, 4 ready");
    expect(describeCounts({})).toBe("empty");
  });

  it("calls a folder ready only when it holds files and all of them are ready", () => {
    expect(allReady({ clean: 5 })).toBe(true);
    expect(allReady({ clean: 5, unknown: 1 })).toBe(false);
    expect(allReady({ clean: 5, missing: 1 })).toBe(false);
    expect(allReady({ clean: 5, embed_open: 1 })).toBe(false);
    expect(allReady({})).toBe(false);
  });
});

describe("ColumnsView", () => {
  it("starts with the libraries and marks the folder that holds a problem", async () => {
    const { api } = makeApi();
    const { view } = show(api);

    await view.findByText("Film");
    const libraries = view.getByRole("listbox", { name: "Libraries" });
    expect(within(libraries).getByText("Film")).toBeInTheDocument();
    expect(within(libraries).getByLabelText("1 blocker, 2 warnings, 3 ready")).toBeInTheDocument();
    expect(within(libraries).getByText("TV").closest("[role=option]")).not.toHaveTextContent("1");
    expect(within(libraries).getByLabelText("2 ready")).toBeInTheDocument();
  });

  it("opens a folder into the next column and a file into the inspector", async () => {
    const { api } = makeApi();
    const { view, onOpen } = show(api);

    await fireEvent.click(await view.findByText("Film"));
    await fireEvent.click(await view.findByText("deutsch"));
    await fireEvent.click(await view.findByText("Film A (2020)"));
    await fireEvent.click(await view.findByText("A.mp4"));

    expect(onOpen).toHaveBeenLastCalledWith("m_A.mp4");
    const crumbs = view.getByRole("navigation", { name: "Location" });
    expect(crumbs).toHaveTextContent("LibrariesFilmdeutschFilm A (2020)");
    expect(view.getByRole("status")).toHaveTextContent("2 files");
  });

  it("walks with the arrow keys: right goes in, down moves, left goes back", async () => {
    const { api } = makeApi();
    const { view, onOpen } = show(api);
    const group = await view.findByRole("group", { name: /Files by folder/ });
    await view.findByText("Film");

    await fireEvent.click(await view.findByText("Film"));
    await view.findByText("deutsch");
    await fireEvent.keyDown(group, { key: "ArrowRight" });
    await waitFor(() => expect(option(view, "deutsch")).toHaveAttribute("aria-selected", "true"));
    await fireEvent.keyDown(group, { key: "ArrowRight" });
    await view.findByText("Film B (2021)");
    await waitFor(() => expect(option(view, "Film A (2020)")).toHaveAttribute("aria-selected", "true"));
    await fireEvent.keyDown(group, { key: "ArrowDown" });
    await waitFor(() => expect(option(view, "Film B (2021)")).toHaveAttribute("aria-selected", "true"));
    await fireEvent.keyDown(group, { key: "ArrowLeft" });
    await waitFor(() => expect(option(view, "deutsch")).toHaveAttribute("aria-selected", "true"));
    // Back in the folder column; the column that showed its subfolder's contents is gone.
    await waitFor(() => expect(view.getAllByRole("listbox")).toHaveLength(3));
    expect(onOpen).not.toHaveBeenCalledWith("m_A.mp4");
  });

  it("goes back to a folder from the path bar", async () => {
    const { api } = makeApi();
    const { view } = show(api);
    await fireEvent.click(await view.findByText("Film"));
    await fireEvent.click(await view.findByText("deutsch"));
    await view.findByText("Film B (2021)");

    await fireEvent.click(within(view.getByRole("navigation", { name: "Location" })).getByRole("button", { name: "Film" }));

    await waitFor(() => expect(option(view, "Film B (2021)")).toBeUndefined());
    expect(option(view, "deutsch")).toBeDefined();
  });

  it("passes the state filter on to every column", async () => {
    const { api, calls } = makeApi();
    show(api, { filter: { ...EMPTY_FILTER, status: ["blocker"] } });

    await waitFor(() => expect(calls[0]).toBe("/browse?status=blocker"));
  });

  it("passes extension and subtitle files on, and shows only the chosen libraries", async () => {
    const { api, calls } = makeApi();
    const { view } = show(api, { filter: { ...EMPTY_FILTER, root: ["r_2"], suffix: [".mkv"], sidecar: true, q: "ignored" } });

    await waitFor(() => expect(calls[0]).toBe("/browse?suffix=.mkv&has_sidecar=true"));
    await waitFor(() => expect(option(view, "TV")).toBeDefined());
    expect(option(view, "Film")).toBeUndefined();
  });

  it("says so when a folder has nothing in the chosen state", async () => {
    const api: Api = { request: vi.fn(async () => new Response(JSON.stringify({ data: { folders: [], files: [] }, meta: {} }))) };
    const { view } = show(api, { filter: { ...EMPTY_FILTER, status: ["blocker"] } });

    expect(await view.findByText("No files in this state")).toBeInTheDocument();
  });

  it("offers a retry when a folder cannot be read", async () => {
    const api: Api = { request: vi.fn(async () => new Response("{}", { status: 500 })) };
    const { view } = show(api);

    expect(await view.findByText("Unable to load this folder")).toBeInTheDocument();
    expect(view.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});

describe("ColumnsView: selecting like the Finder", () => {
  const inFilmA = async (over: Partial<Parameters<typeof ColumnsView>[0]> = {}) => {
    // The page sets this once a file is open; a truthy value lets the view close the inspector again.
    const { api } = makeApi();
    const picks: Array<{ files: string[]; folders: string[] }> = [];
    const actions: string[] = [];
    const shown = show(api, { activeId: "open", onSelection: (picked) => picks.push({ files: picked.files.map((f) => f.id), folders: picked.folders.map((f) => f.path) }), onAction: (action) => actions.push(action), ...over });
    await fireEvent.click(await shown.view.findByText("Film"));
    await fireEvent.click(await shown.view.findByText("deutsch"));
    await fireEvent.click(await shown.view.findByText("Film A (2020)"));
    await shown.view.findByText("A 2.mp4");
    return { ...shown, picks, actions };
  };
  const row = (view: ReturnType<typeof render>, name: string) => option(view, name) as HTMLElement;
  const selected = (view: ReturnType<typeof render>) =>
    Array.from(view.container.querySelectorAll<HTMLElement>("[role=option][aria-selected=true]"))
      .map((el) => el.querySelector(".col-name")?.textContent ?? "")
      .filter((name) => name.endsWith(".mp4"));

  it("adds and removes single files with Cmd or Ctrl, and tells the page", async () => {
    const { view, picks, onOpen } = await inFilmA();

    await fireEvent.click(row(view, "A.mp4"));
    expect(onOpen).toHaveBeenLastCalledWith("m_A.mp4");
    await fireEvent.click(row(view, "A 2.mp4"), { metaKey: true });
    expect(selected(view)).toEqual(expect.arrayContaining(["A.mp4", "A 2.mp4"]));
    expect(picks.at(-1)?.files).toEqual(["m_A.mp4", "m_A 2.mp4"]);
    // Several files are no longer "the open file": the inspector is closed.
    expect(onOpen).toHaveBeenLastCalledWith(undefined);

    await fireEvent.click(row(view, "A.mp4"), { ctrlKey: true });
    await waitFor(() => expect(picks.at(-1)?.files).toEqual(["m_A 2.mp4"]));
  });

  it("takes a range with Shift, from where the click began", async () => {
    const { view, picks } = await inFilmA();

    await fireEvent.click(row(view, "A.mp4"));
    await fireEvent.click(row(view, "A 2.mp4"), { shiftKey: true });

    expect(selected(view).sort()).toEqual(["A 2.mp4", "A.mp4"]);
    expect(picks.at(-1)?.files).toHaveLength(2);
  });

  it("selects everything in the column with Cmd+A and lets Escape let go", async () => {
    const { view, picks } = await inFilmA();
    const group = view.getByRole("group", { name: /Files by folder/ });

    await fireEvent.click(row(view, "A.mp4"));
    await fireEvent.keyDown(group, { key: "a", metaKey: true });
    expect(picks.at(-1)?.files).toHaveLength(2);

    await fireEvent.keyDown(group, { key: "Escape" });
    await waitFor(() => expect(selected(view)).not.toContain("A.mp4"));
    expect(picks.at(-1)?.files).toEqual([]);
  });

  it("extends the selection with Shift and the arrow keys", async () => {
    const { view, picks } = await inFilmA();
    const group = view.getByRole("group", { name: /Files by folder/ });

    await fireEvent.click(row(view, "A.mp4"));
    await fireEvent.keyDown(group, { key: "ArrowDown", shiftKey: true });

    await waitFor(() => expect(picks.at(-1)?.files).toHaveLength(2));
  });

  it("opens a context menu for the selection and runs what is chosen", async () => {
    const { view, actions } = await inFilmA();

    await fireEvent.click(row(view, "A.mp4"));
    await fireEvent.click(row(view, "A 2.mp4"), { metaKey: true });
    await fireEvent.contextMenu(row(view, "A 2.mp4"), { clientX: 120, clientY: 140 });

    const menu = within(document.body).getByRole("menu", { name: "Actions for the selection" });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "Verify 2 itemsV", "Prepare preview…F", "Re-inspect 2 itemsI", "Copy names", "Copy paths", expect.stringMatching(/^Select all/),
    ]);
    await fireEvent.click(within(menu).getByRole("menuitem", { name: /Verify 2 items/ }));

    expect(actions).toEqual(["verify"]);
    expect(within(document.body).queryByRole("menu")).not.toBeInTheDocument();
  });

  it("acts on the row under a right click when it is outside the selection, like the Finder", async () => {
    const { view, picks } = await inFilmA();

    await fireEvent.click(row(view, "A.mp4"));
    await fireEvent.contextMenu(row(view, "A 2.mp4"));

    expect(picks.at(-1)?.files).toEqual(["m_A 2.mp4"]);
    expect(selected(view)).toEqual(expect.arrayContaining(["A 2.mp4"]));
    expect(within(document.body).getByRole("menuitem", { name: /Verify file/ })).toBeInTheDocument();
    await fireEvent.keyDown(within(document.body).getByRole("menu"), { key: "Escape" });
    expect(within(document.body).queryByRole("menu")).not.toBeInTheDocument();
  });

  it("reports folders so the page can reach the files below them", async () => {
    const { api } = makeApi();
    const picks: Array<Array<{ name: string; path: string; rootId: string | null }>> = [];
    const { view } = show(api, { onSelection: (picked) => picks.push(picked.folders) });
    await fireEvent.click(await view.findByText("Film"));
    await fireEvent.click(await view.findByText("deutsch"));
    await view.findByText("Film B (2021)");

    await fireEvent.click(row(view, "Film A (2020)"));
    await fireEvent.click(row(view, "Film B (2021)"), { metaKey: true });

    expect(picks.at(-1)).toEqual([
      { name: "Film A (2020)", rootId: "r_1", path: "deutsch/Film A (2020)" },
      { name: "Film B (2021)", rootId: "r_1", path: "deutsch/Film B (2021)" },
    ]);
  });

  it("clears the selection when the page asks", async () => {
    const { api } = makeApi();
    const picks: number[] = [];
    const { view } = show(api, { clearToken: 0, onSelection: (picked) => picks.push(picked.files.length + picked.folders.length) });
    await fireEvent.click(await view.findByText("Film"));
    expect(picks.at(-1)).toBe(1);
  });
});

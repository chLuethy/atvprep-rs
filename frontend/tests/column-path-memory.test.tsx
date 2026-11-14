import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../src/App";

// The column view keeps the opened folder path, and the list or columns choice, across a page change and a reload.
const LEVELS: Record<string, unknown> = {
  "": { folders: [{ name: "Film", id: "r_1", counts: { clean: 3 }, total: 3 }, { name: "TV", id: "r_2", counts: { clean: 2 }, total: 2 }], files: [] },
  "r_1|": { folders: [{ name: "deutsch", counts: { clean: 3 }, total: 3 }], files: [] },
  "r_1|deutsch": { folders: [{ name: "Film A (2020)", counts: { clean: 2 }, total: 2 }, { name: "Film B (2021)", counts: { clean: 1 }, total: 1 }], files: [] },
  "r_1|deutsch/Film A (2020)": { folders: [], files: [] },
  "r_1|deutsch/Film B (2021)": { folders: [], files: [] },
};

describe("the column path survives a page change", () => {
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

  const fakeApi = (levels: Record<string, unknown> = LEVELS) => ({
    request: vi.fn(async (path: string) => {
      if (path.startsWith("/browse")) {
        const query = new URLSearchParams(path.split("?")[1] ?? "");
        const key = query.has("root_id") ? `${query.get("root_id")}|${query.get("path") ?? ""}` : "";
        const level = levels[key];
        return level ? new Response(JSON.stringify({ data: level, meta: {} })) : new Response("{}", { status: 404 });
      }
      if (path.startsWith("/files?") || path.startsWith("/jobs?")) return new Response(JSON.stringify({ data: [], meta: { next_cursor: null } }));
      if (path === "/roots") return new Response(JSON.stringify({ data: [{ id: "r_1", name: "Film" }, { id: "r_2", name: "TV" }] }));
      return new Response(JSON.stringify({ data: {} }));
    }),
  });

  const nav = (view: ReturnType<typeof render>, name: RegExp) =>
    within(view.getByRole("navigation", { name: /main/i })).getByRole("link", { name });
  const option = (view: ReturnType<typeof render>, name: string) =>
    Array.from(view.container.querySelectorAll<HTMLElement>("[role=option]")).find((row) => row.querySelector(".col-name")?.textContent === name);
  const crumbs = (view: ReturnType<typeof render>) => view.getByRole("navigation", { name: "Location" });

  const openColumns = async (view: ReturnType<typeof render>) => {
    await fireEvent.click(await view.findByRole("button", { name: /Columns/ }));
    await waitFor(() => expect(option(view, "Film")).toBeDefined());
  };
  const openFolders = async (view: ReturnType<typeof render>, names: string[]) => {
    for (const name of names) {
      await waitFor(() => expect(option(view, name)).toBeDefined());
      await fireEvent.click(option(view, name)!);
    }
  };
  const away = async (view: ReturnType<typeof render>) => {
    await fireEvent.click(nav(view, /Activity/));
    await waitFor(() => expect(window.location.pathname).toBe("/jobs"));
    await fireEvent.click(nav(view, /Files/));
    await waitFor(() => expect(window.location.pathname).toBe("/files"));
  };

  it("opens the same folder path again after a visit to Activity", async () => {
    window.history.replaceState(null, "", "/files");
    const view = render(() => <App api={fakeApi() as never} />);
    await openColumns(view);
    await openFolders(view, ["Film", "deutsch", "Film A (2020)"]);
    await waitFor(() => expect(crumbs(view)).toHaveTextContent("LibrariesFilmdeutschFilm A (2020)"));

    await away(view);

    await waitFor(() => expect(crumbs(view)).toHaveTextContent("LibrariesFilmdeutschFilm A (2020)"));
    expect(option(view, "Film A (2020)")).toHaveAttribute("aria-selected", "true");
    expect(view.getAllByRole("listbox")).toHaveLength(4);
    expect(view.getByRole("button", { name: /Columns/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("opens the path again after a reload, together with the remembered filter", async () => {
    window.history.replaceState(null, "", "/files?status=clean");
    const first = render(() => <App api={fakeApi() as never} />);
    await openColumns(first);
    await openFolders(first, ["Film", "deutsch"]);
    await waitFor(() => expect(crumbs(first)).toHaveTextContent("LibrariesFilmdeutsch"));
    first.unmount();

    window.history.replaceState(null, "", "/files");
    const view = render(() => <App api={fakeApi() as never} />);
    await waitFor(() => expect(window.location.search).toBe("?status=clean"));
    await waitFor(() => expect(crumbs(view)).toHaveTextContent("LibrariesFilmdeutsch"));
    await waitFor(() => expect(option(view, "Film B (2021)")).toBeDefined());
    expect(option(view, "deutsch")).toHaveAttribute("aria-selected", "true");
    expect(view.getByRole("button", { name: /Columns/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("ends at the deepest folder still there when one of the path is gone", async () => {
    window.history.replaceState(null, "", "/files");
    const first = render(() => <App api={fakeApi() as never} />);
    await openColumns(first);
    await openFolders(first, ["Film", "deutsch", "Film A (2020)"]);
    await waitFor(() => expect(crumbs(first)).toHaveTextContent("LibrariesFilmdeutschFilm A (2020)"));
    first.unmount();

    const gone = { ...LEVELS, "r_1|deutsch": { folders: [{ name: "Film B (2021)", counts: { clean: 1 }, total: 1 }], files: [] } };
    const view = render(() => <App api={fakeApi(gone) as never} />);
    await waitFor(() => expect(option(view, "Film B (2021)")).toBeDefined());
    await waitFor(() => expect(option(view, "deutsch")).toHaveAttribute("aria-selected", "true"));
    expect(crumbs(view)).toHaveTextContent(/^LibrariesFilmdeutsch$/);
    expect(view.queryByRole("alert")).not.toBeInTheDocument();
    expect(option(view, "Film B (2021)")).toHaveAttribute("aria-selected", "false");
  });

  it("keeps the list or columns choice across a page change and a reload", async () => {
    window.history.replaceState(null, "", "/files");
    const first = render(() => <App api={fakeApi() as never} />);
    await openColumns(first);
    await away(first);
    expect(await first.findByRole("button", { name: /Columns/ })).toHaveAttribute("aria-pressed", "true");
    await fireEvent.click(first.getByRole("button", { name: /List/ }));
    await away(first);
    expect(await first.findByRole("button", { name: /List/ })).toHaveAttribute("aria-pressed", "true");
    await fireEvent.click(first.getByRole("button", { name: /Columns/ }));
    first.unmount();

    const view = render(() => <App api={fakeApi() as never} />);
    expect(await view.findByRole("button", { name: /Columns/ })).toHaveAttribute("aria-pressed", "true");
    await waitFor(() => expect(option(view, "Film")).toBeDefined());
  });

  it("forgets the path on a filter change and does not apply a path kept under another filter", async () => {
    window.history.replaceState(null, "", "/files");
    const first = render(() => <App api={fakeApi() as never} />);
    await openColumns(first);
    await openFolders(first, ["Film", "deutsch"]);
    await waitFor(() => expect(crumbs(first)).toHaveTextContent("LibrariesFilmdeutsch"));

    const bar = await first.findByRole("search", { name: "Filter files" });
    await fireEvent.click(within(bar).getByRole("button", { name: /^Status$/ }));
    await fireEvent.click(within(first.getByRole("dialog", { name: "Status" })).getByRole("checkbox", { name: /^Warning/ }));
    await waitFor(() => expect(window.location.search).toBe("?status=warning"));
    await waitFor(() => expect(crumbs(first)).toHaveTextContent(/^Libraries$/));
    await fireEvent.keyDown(document, { key: "Escape" });
    window.history.back();
    await waitFor(() => expect(window.location.search).toBe(""));
    await waitFor(() => expect(option(first, "Film")).toBeDefined());
    expect(crumbs(first)).toHaveTextContent(/^Libraries$/);
    first.unmount();

    // A path kept under no filter is not opened under another one.
    const second = render(() => <App api={fakeApi() as never} />);
    await openFolders(second, ["Film", "deutsch"]);
    await waitFor(() => expect(crumbs(second)).toHaveTextContent("LibrariesFilmdeutsch"));
    second.unmount();
    window.history.replaceState(null, "", "/files?status=blocker");
    const view = render(() => <App api={fakeApi() as never} />);
    await waitFor(() => expect(option(view, "Film")).toBeDefined());
    await waitFor(() => expect(view.getAllByRole("listbox")).toHaveLength(1));
    expect(crumbs(view)).toHaveTextContent(/^Libraries$/);
  });
});

import { fireEvent, render, waitFor } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { FileTable } from "../src/features/files/FileTable";
import { LibraryPage } from "../src/features/files/LibraryPage";
import { ColumnsView } from "../src/features/files/ColumnsView";
import { EMPTY_FILTER } from "../src/features/files/filters";
import type { FileRow } from "../src/features/files/loaders";
import { withServerState } from "./fakeServerState";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

const row = (index: number): FileRow => ({
  id: `m_${index}`, root_id: "r_1", display_path: `TV/Serie (2017)/Season 01/Serie - S01E${String(index).padStart(4, "0")}.mp4`, kind: "episode", suffix: ".mp4",
  size: 1000 + index, mtime_ns: 1, discovery_state: "present", compliance_state: "clean", state: "clean", preparable: false,
  workflow_state: "idle", desired_action: "none", last_seen_at: null,
});
const json = (data: unknown, meta: unknown = {}) => new Response(JSON.stringify({ data: withServerState(data), meta }), { status: 200 });

describe("grosse Listen", () => {
  it("rendert von 2000 Zeilen nur ein Fenster und behält die Gesamtzahl für Hilfstechnik", () => {
    const rows = Array.from({ length: 2000 }, (_, index) => row(index));
    const view = render(() => (
      <FileTable rows={rows} activeId={undefined} picked={new Set()} sort={null} onSort={() => {}} onSelect={() => {}} onMenu={() => {}} />
    ));
    const rendered = view.container.querySelectorAll("tr.row").length;
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThanOrEqual(120);
    expect(view.container.querySelector("table")).toHaveAttribute("aria-rowcount", "2001");
    expect(view.container.querySelector("tr.row")).toHaveAttribute("aria-rowindex", "2");
  });

  it("J führt über das Fenster hinaus: die Zeile wird gebaut und bekommt den Fokus", async () => {
    const first = Array.from({ length: 50 }, (_, index) => row(index));
    const second = Array.from({ length: 50 }, (_, index) => row(50 + index));
    const api: Api = { request: vi.fn(async (path: string) => {
      if (path === "/dashboard") return json({ files: {}, scans: {}, jobs: {} });
      if (path.startsWith("/files?")) return path.includes("cursor=c2") ? json(second, { next_cursor: null }) : json(first, { next_cursor: "c2" });
      return json([]);
    }) };
    const view = render(() => <LibraryPage api={api} activeId={undefined} onOpen={() => {}} />);
    fireEvent.click(await view.findByRole("button", { name: "Load more files" }));
    await waitFor(() => expect(vi.mocked(api.request).mock.calls.some((call) => call[0].includes("cursor=c2"))).toBe(true));
    const start = await view.findByRole("link", { name: /S01E0000/ });
    (start.closest("tr") as HTMLElement).focus();
    for (let step = 1; step <= 80; step += 1) {
      fireEvent.keyDown(document.activeElement as HTMLElement, { key: "j" });
      await waitFor(() => expect((document.activeElement as HTMLElement).dataset.id).toBe(`m_${step}`));
    }
    expect(view.container.querySelectorAll("tr.row").length).toBeLessThanOrEqual(120);
  });

  it("eine Auffrischung liest nur eine Seite statt jeder geladenen", async () => {
    vi.useFakeTimers();
    try {
      let reads = 0;
      const api: Api = { request: vi.fn(async (path: string) => {
        if (path === "/dashboard") return json({ files: {}, scans: {}, jobs: { running: 1 } });
        if (!path.startsWith("/files?")) return json([]);
        if (path.includes("cursor=c2")) return json(Array.from({ length: 50 }, (_, index) => row(50 + index)), { next_cursor: "c3" });
        if (path.includes("cursor=c3")) return json(Array.from({ length: 50 }, (_, index) => row(100 + index)), { next_cursor: null });
        reads += 1;
        return json(Array.from({ length: 50 }, (_, index) => row(index)), { next_cursor: "c2" });
      }) };
      const view = render(() => <LibraryPage api={api} activeId={undefined} onOpen={() => {}} />);
      await vi.advanceTimersByTimeAsync(0);
      fireEvent.click(view.getByRole("button", { name: "Load more files" }));
      await vi.advanceTimersByTimeAsync(0);
      fireEvent.click(view.getByRole("button", { name: "Load more files" }));
      await vi.advanceTimersByTimeAsync(0);
      const before = vi.mocked(api.request).mock.calls.length;
      await vi.advanceTimersByTimeAsync(4100);
      const calls = vi.mocked(api.request).mock.calls.slice(before).map((call) => call[0]).filter((path) => path.startsWith("/files?"));
      expect(calls).toHaveLength(1);
      expect(reads).toBeGreaterThanOrEqual(2);
      // The pages loaded later stay in the list.
      expect(view.getByRole("table")).toHaveAttribute("aria-rowcount", "151");
      view.unmount();
    } finally { vi.useRealTimers(); }
  });

  it("die Spaltenansicht rendert einen grossen Ordner nicht ganz", async () => {
    const files = Array.from({ length: 400 }, (_, index) => row(index));
    const api: Api = { request: vi.fn(async (path: string) => {
      const query = new URLSearchParams(path.split("?")[1] ?? "");
      if (!query.has("root_id")) return json({ folders: [{ name: "TV", id: "r_1", counts: { clean: 400 }, total: 400 }], files: [] });
      return json({ folders: [], files });
    }) };
    const view = render(() => <ColumnsView api={api} filter={EMPTY_FILTER} version={0} onOpen={() => {}} />);
    fireEvent.click(await view.findByText("TV"));
    await waitFor(() => expect(view.container.querySelectorAll('.col[data-col="1"] .col-row').length).toBeGreaterThan(0));
    expect(view.container.querySelectorAll('.col[data-col="1"] .col-row').length).toBeLessThanOrEqual(120);
  });
});

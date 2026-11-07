import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { OverviewPage } from "../src/features/overview/OverviewPage";

const summary = (files: Record<string, number>, jobs: Record<string, number> = {}) => ({ data: { files, scans: {}, jobs } });
const api = (responses: Record<string, unknown>, status = 200) => ({
  request: vi.fn(async (path: string) => new Response(JSON.stringify(responses[path] ?? {}), { status })),
});
const base = (over: Record<string, unknown> = {}) => ({
  "/dashboard": summary({ clean: 12, warning: 3, blocker: 1, unknown: 2, probe_failed: 1 }, { failed: 2, running: 1 }),
  "/dashboard/activity?limit=8": { data: [{ id: "e1", kind: "job_event", topic: "job.created" }, { id: "e2", kind: "job_event", topic: "job.retry_created" }, { id: "e3", kind: "job_event", topic: "something.else" }] },
  "/dashboard/queue?limit=5": { data: { total: 2, jobs: [{ id: "j1", action: "verify", status: "running", total_count: 3, succeeded_count: 1, failed_count: 0, cancelled_count: 0 }] } },
  ...over,
});

describe("OverviewPage: library health", () => {
  it("draws the library as one bar with a spoken summary and a legend that links to each status", async () => {
    const view = render(() => <OverviewPage api={api(base()) as never} />);

    expect(view.getByText("Loading overview")).toBeInTheDocument();
    const health = await view.findByRole("region", { name: "Library health" });
    expect(within(health).getByRole("img")).toHaveAccessibleName("19 files: 12 ready, 3 warning, 1 blocker, 2 not verified, 1 inspection failed");
    expect(within(health).getByRole("link", { name: "12 Ready" })).toHaveAttribute("href", "/files?status=clean");
    expect(within(health).getByRole("link", { name: "1 Blocker" })).toHaveAttribute("href", "/files?status=blocker");
    expect(within(health).getByRole("link", { name: "2 Not verified" })).toHaveAttribute("href", "/files?status=unknown");
  });

  it("says so when the library is empty", async () => {
    const view = render(() => <OverviewPage api={api(base({ "/dashboard": summary({}) })) as never} />);

    expect(await view.findByText(/no files yet/i)).toBeInTheDocument();
  });

  it("offers a scan right where it says there are no files", async () => {
    const roots = { data: [{ id: "r_1", name: "Film", latest_scan: null }] };
    const view = render(() => <OverviewPage api={api(base({ "/dashboard": summary({}), "/roots": roots })) as never} />);

    const health = await view.findByRole("region", { name: "Library health" });

    expect(await within(health).findByRole("button", { name: "Scan library" })).toBeEnabled();
  });
});

describe("OverviewPage: when everything is ready", () => {
  const allClean = () => base({ "/dashboard": summary({ clean: 19 }) });

  it("says so in words that name the count and the goal, instead of a bare nothing", async () => {
    window.localStorage.clear();
    const view = render(() => <OverviewPage api={api(allClean()) as never} />);

    const health = await view.findByRole("region", { name: "Library health" });

    expect(within(health).getByText("All 19 files are ready for Apple TV.")).toBeInTheDocument();
    expect(await view.findByText("Nothing needs you.")).toBeInTheDocument();
  });

  it("draws the check the first time it is seen that day and shows it plain afterwards", async () => {
    window.localStorage.clear();
    const first = render(() => <OverviewPage api={api(allClean()) as never} />);
    const drawn = (await first.findByRole("region", { name: "Library health" })).querySelector(".ready-check");
    expect(drawn).toHaveClass("draw");
    first.unmount();

    const second = render(() => <OverviewPage api={api(allClean()) as never} />);
    const plain = (await second.findByRole("region", { name: "Library health" })).querySelector(".ready-check");
    expect(plain).not.toHaveClass("draw");
    expect(plain).toHaveAttribute("aria-hidden", "true");
  });

  it("says it for a single file in the singular", async () => {
    window.localStorage.clear();
    const view = render(() => <OverviewPage api={api(base({ "/dashboard": summary({ clean: 1 }) })) as never} />);

    expect(await view.findByText("The file is ready for Apple TV.")).toBeInTheDocument();
  });

  it("does not celebrate while something still needs attention", async () => {
    window.localStorage.clear();
    const view = render(() => <OverviewPage api={api(base()) as never} />);

    const health = await view.findByRole("region", { name: "Library health" });

    expect(health.querySelector(".ready-check")).toBeNull();
  });
});

describe("OverviewPage: the re-encode count", () => {
  it("shows how many files need a video re-encode and links to them", async () => {
    const view = render(() => <OverviewPage api={api(base({ "/dashboard": { data: { files: { clean: 3, blocker: 2 }, scans: {}, jobs: {}, reencode: 2 } } })) as never} />);

    expect(await view.findByRole("link", { name: "2 Need a video re-encode" })).toHaveAttribute("href", "/files?status=reencode");
  });
});

describe("OverviewPage: what needs you", () => {
  it("lists what needs attention, worst first, in plain words with the right number", async () => {
    const view = render(() => <OverviewPage api={api(base()) as never} />);

    const needs = await view.findByRole("region", { name: "Needs you" });
    const rows = within(needs).getAllByRole("link").map((link) => link.textContent);
    expect(rows).toEqual([
      "1 file has a blocker",
      "3 files have warnings",
      "2 files are not verified",
      "1 file could not be inspected",
      "2 jobs failed",
    ]);
    expect(within(needs).getByRole("link", { name: "3 files have warnings" })).toHaveAttribute("href", "/files?status=warning");
    expect(within(needs).getByRole("link", { name: "2 jobs failed" })).toHaveAttribute("href", "/jobs");
  });

  it("says plainly when nothing needs attention", async () => {
    const view = render(() => <OverviewPage api={api(base({ "/dashboard": summary({ clean: 5 }) })) as never} />);

    expect(await view.findByText("Nothing needs you.")).toBeInTheDocument();
  });

  it("hands navigation to the app when it can, and otherwise stays a plain link", async () => {
    const onNavigate = vi.fn();
    const view = render(() => <OverviewPage api={api(base()) as never} onNavigate={onNavigate} />);

    const link = await view.findByRole("link", { name: "1 file has a blocker" });
    const allowed = fireEvent.click(link);

    expect(onNavigate).toHaveBeenCalledWith("/files?status=blocker");
    expect(allowed).toBe(false);
  });
});

describe("OverviewPage: running and recent", () => {
  it("shows what is running now with its progress", async () => {
    const view = render(() => <OverviewPage api={api(base()) as never} />);

    const now = await view.findByRole("region", { name: "Running now" });
    expect(within(now).getByRole("link", { name: /Verify · 1 of 3 succeeded/ })).toHaveAttribute("href", "/jobs/j1");
    expect(within(now).getByText("Running")).toBeInTheDocument();
    expect(within(now).getByText("2 active jobs")).toBeInTheDocument();
  });

  it("names the file a running job is about when the server says which", async () => {
    const view = render(() => <OverviewPage api={api(base({
      "/dashboard/queue?limit=5": { data: { total: 1, jobs: [{ id: "j2", action: "fix", status: "queued", total_count: 2, succeeded_count: 0, failed_count: 0, cancelled_count: 0, subject: "Film/Heat (1995)/Heat.mp4" }] } },
    })) as never} />);

    const now = await view.findByRole("region", { name: "Running now" });
    expect(within(now).getByRole("link", { name: "Fix: Heat and 1 more · 0 of 2 succeeded" })).toHaveAttribute("href", "/jobs/j2");
    expect(within(now).getByText("1 active job")).toBeInTheDocument();
  });

  it("says when nothing is running or has happened", async () => {
    const view = render(() => <OverviewPage api={api(base({
      "/dashboard/activity?limit=8": { data: [] },
      "/dashboard/queue?limit=5": { data: { total: 0, jobs: [] } },
    })) as never} />);

    expect(await view.findByText("Nothing is running.")).toBeInTheDocument();
    expect(view.getByText("No activity yet")).toBeInTheDocument();
  });

  it("names recent events in words and keeps an unknown topic as it is", async () => {
    const view = render(() => <OverviewPage api={api(base()) as never} />);

    const recent = await view.findByRole("region", { name: "Recent activity" });
    expect(within(recent).getByText("Job queued")).toBeInTheDocument();
    expect(within(recent).getByText("Retry queued")).toBeInTheDocument();
    expect(within(recent).getByText("something.else")).toBeInTheDocument();
  });
});

describe("OverviewPage: leftover partial rewrites", () => {
  const hit = { root_id: "r_1", name: "Movies", count: 3, bytes: 2_684_354_560, files: [
    { relative_path: "Show/.tmp_fixmeta_enc.Film.4711-0a1b2c3d.mp4", size: 1_073_741_824, modified_at: "2026-11-01T10:00:00" },
  ] };
  const withHits = (extra: Record<string, unknown> = {}) => base({
    "/dashboard": { data: { files: { clean: 3 }, scans: {}, jobs: {}, orphans: [hit] } },
    ...extra,
  });

  it("shows nothing without hits", async () => {
    const view = render(() => <OverviewPage api={api(base()) as never} />);
    await view.findByRole("region", { name: "Library health" });

    expect(view.queryByRole("region", { name: "Leftover files" })).toBeNull();
  });

  it("names count, library, size and the first files when there are hits", async () => {
    const view = render(() => <OverviewPage api={api(withHits()) as never} />);

    const section = await view.findByRole("region", { name: "Leftover files" });

    expect(within(section).getByText("3 leftover files in Movies (2.5 GB)")).toBeInTheDocument();
    expect(within(section).getByText(/Show\/\.tmp_fixmeta_enc\.Film\.4711-0a1b2c3d\.mp4/)).toBeInTheDocument();
    expect(within(section).getByText("and 2 more")).toBeInTheDocument();
  });

  it("deletes only after a confirmation that names count and size, then reloads", async () => {
    let removed = false;
    const calls: Array<{ path: string; init?: RequestInit }> = [];
    const mock = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        calls.push({ path, init });
        if (init?.method === "POST") { removed = true; return new Response(JSON.stringify({ data: { removed: 3, bytes: 2_684_354_560 } })); }
        const responses = withHits({ "/dashboard": { data: { files: { clean: 3 }, scans: {}, jobs: {}, orphans: removed ? [] : [hit] } } });
        return new Response(JSON.stringify(responses[path as keyof typeof responses] ?? {}));
      }),
    };
    const view = render(() => <OverviewPage api={mock as never} />);
    const section = await view.findByRole("region", { name: "Leftover files" });

    fireEvent.click(within(section).getByRole("button", { name: "Remove leftovers" }));
    const confirm = await view.findByRole("region", { name: "Confirm removing leftovers" });
    expect(within(confirm).getByText("Delete 3 files, 2.5 GB in total?")).toBeInTheDocument();
    expect(calls.some((call) => call.init?.method === "POST")).toBe(false);

    fireEvent.click(within(confirm).getByRole("button", { name: "Delete leftovers" }));

    expect(await view.findByText("Removed 3 leftover files (2.5 GB).")).toBeInTheDocument();
    const post = calls.find((call) => call.init?.method === "POST");
    expect(post?.path).toBe("/roots/r_1/orphans/remove");
    expect(JSON.parse(String(post?.init?.body))).toEqual({ count: 3, bytes: 2_684_354_560 });
    await waitFor(() => expect(view.queryByRole("region", { name: "Leftover files" })).toBeNull());
  });

  it("keeps everything when the operator keeps the files", async () => {
    const responses = withHits();
    const mock = { request: vi.fn(async (path: string, _init?: RequestInit) => new Response(JSON.stringify(responses[path as keyof typeof responses] ?? {}))) };
    const view = render(() => <OverviewPage api={mock as never} />);
    const section = await view.findByRole("region", { name: "Leftover files" });

    fireEvent.click(within(section).getByRole("button", { name: "Remove leftovers" }));
    fireEvent.click(await view.findByRole("button", { name: "Keep" }));

    expect(view.queryByRole("region", { name: "Confirm removing leftovers" })).toBeNull();
    expect(mock.request.mock.calls.some(([, init]) => (init as RequestInit | undefined)?.method === "POST")).toBe(false);
  });

  it("says so when the files changed since the scan", async () => {
    const mock = {
      request: vi.fn(async (path: string, init?: RequestInit) => {
        if (init?.method === "POST") return new Response(JSON.stringify({ data: { count: 4, bytes: 5 } }), { status: 409 });
        const responses = withHits();
        return new Response(JSON.stringify(responses[path as keyof typeof responses] ?? {}));
      }),
    };
    const view = render(() => <OverviewPage api={mock as never} />);
    const section = await view.findByRole("region", { name: "Leftover files" });

    fireEvent.click(within(section).getByRole("button", { name: "Remove leftovers" }));
    fireEvent.click(await view.findByRole("button", { name: "Delete leftovers" }));

    expect(await view.findByRole("alert")).toHaveTextContent("The leftovers changed since the scan. Check the new numbers and try again.");
  });

  it("names the cleanup in recent activity", async () => {
    const view = render(() => <OverviewPage api={api(base({
      "/dashboard/activity?limit=8": { data: [{ id: "e1", kind: "job_event", topic: "orphan.cleanup" }] },
    })) as never} />);

    const recent = await view.findByRole("region", { name: "Recent activity" });
    expect(within(recent).getByText("Leftover files removed")).toBeInTheDocument();
  });
});

describe("OverviewPage: failure", () => {
  it("shows a retryable error", async () => {
    const view = render(() => <OverviewPage api={api({}, 503) as never} />);

    expect(await view.findByRole("alert")).toHaveTextContent("Unable to load overview");
    expect(view.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});

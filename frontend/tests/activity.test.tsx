import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActivityPage } from "../src/features/activity/ActivityPage";
import { JobDetail } from "../src/features/activity/JobDetail";
import { LogViewer } from "../src/features/activity/LogViewer";
import { SessionError } from "../src/lib/api";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };
const json = (data: unknown, status = 200, meta: unknown = {}) => new Response(JSON.stringify({ data, meta }), { status });
const apiFor = (handler: (path: string, init?: RequestInit) => Response | Promise<Response>): Api => ({
  request: vi.fn(async (path: string, init?: RequestInit) => handler(path, init)),
});
const calls = (api: Api) => (api.request as unknown as ReturnType<typeof vi.fn>).mock.calls as Array<[string, RequestInit | undefined]>;

const job = (over: Record<string, unknown> = {}) => ({
  id: "j_1", action: "fix", status: "succeeded", trigger: "manual", origin_job_id: null,
  requested_at: "2026-07-20T10:00:00Z", started_at: "2026-07-20T10:01:00Z", finished_at: "2026-07-20T10:02:00Z",
  total_count: 1, succeeded_count: 1, failed_count: 0, cancelled_count: 0, subject: "Film/Heat (1995)/Heat.mp4", ...over,
});
const item = (over: Record<string, unknown> = {}) => ({
  id: "i_1", job_id: "j_1", media_id: "m_1", action: "fix", position: 1, status: "succeeded", attempt: 1,
  output_media_id: null, display_path: "Film/Heat (1995)/Heat.mp4", error_code: null, error_detail: null, ...over,
});
afterEach(() => { vi.useRealTimers(); });

describe("ActivityPage: after a retry", () => {
  it("opens the new job and says why the view changed", async () => {
    const onOpen = vi.fn();
    const api = apiFor((path, init) => {
      if (path === "/jobs/j_1/retry" && init?.method === "POST") return json({ id: "j_new", status: "queued" });
      if (path === "/jobs/j_1") return json(job({ status: "failed", succeeded_count: 0, failed_count: 1 }));
      if (path.startsWith("/jobs/j_1/items")) return json([item({ status: "failed", error_code: "x", error_detail: "y" })]);
      if (path.startsWith("/jobs?")) return json([job({ status: "failed", succeeded_count: 0, failed_count: 1 })]);
      return json([]);
    });
    const view = render(() => <ActivityPage api={api} activeId="j_1" onOpen={onOpen} />);

    await fireEvent.click(await view.findByRole("button", { name: "Retry failed items" }));
    await fireEvent.click(view.getByRole("button", { name: "Confirm retry" }));

    await waitFor(() => expect(onOpen).toHaveBeenCalledWith("j_new"));
    expect(await view.findByText(/retry queued.*showing the new job/i)).toBeInTheDocument();
  });
});

describe("ActivityPage: the list", () => {
  it("asks for the newest jobs first and names each by its file, action and progress", async () => {
    const api = apiFor((path) => path.startsWith("/jobs?") ? json([job({ id: "j_9", total_count: 2, succeeded_count: 1, failed_count: 1, status: "partial_success" })]) : json([]));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);

    expect(await view.findByText("Heat and 1 more")).toBeInTheDocument();
    expect(view.getByText(/Fix · 1 of 2 succeeded, 1 failed/)).toBeInTheDocument();
    expect(within(view.getByRole("region", { name: "History" })).getByText("Partly failed")).toBeInTheDocument();
    expect(calls(api)[0]![0]).toBe("/jobs?limit=50&order=desc&exclude_status=cancelled");
  });

  it("shows how far a running job has come and for how long, and nothing for the others", async () => {
    const started = new Date(Date.now() - 134_000).toISOString();
    const api = apiFor(() => json([
      job({ id: "j_3", status: "running", progress: 42, started_at: started, finished_at: null, subject: "Film/Sisu (2025)/Sisu (2025).mp4" }),
      job({ id: "j_2", status: "queued", progress: 0, started_at: null, finished_at: null }),
      job({ id: "j_1", status: "succeeded", progress: 100 }),
    ]));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);

    const bars = await view.findAllByRole("progressbar");

    expect(bars).toHaveLength(1);
    expect(bars[0]).toHaveAttribute("aria-valuenow", "42");
    expect(bars[0]).toHaveAccessibleName(/progress.*sisu/i);
    const row = bars[0]!.closest("li") as HTMLElement;
    expect(row).toHaveTextContent("42%");
    expect(row).toHaveTextContent(/running for 2 min/i);
  });

  it("rebuilds only the job rows that changed when the list refreshes under new ids", async () => {
    vi.useFakeTimers();
    let serial = 0;
    let progress = 20;
    const api = apiFor(() => {
      serial += 1;
      return json([
        job({ id: `j_r${serial}`, status: "running", progress, requested_at: "2026-07-20T10:05:00Z", started_at: "2026-07-20T10:05:01Z", finished_at: null, subject: "Film/Running (2025)/Running (2025).mp4" }),
        job({ id: `j_d${serial}`, status: "succeeded", requested_at: "2026-07-20T09:00:00Z", subject: "Film/Done (2024)/Done (2024).mp4" }),
      ]);
    });
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    const rowOf = (name: string) => view.getByText(name).closest("li") as HTMLElement;
    const done = rowOf("Done (2024)");

    progress = 60;
    await vi.advanceTimersByTimeAsync(4100);

    expect(rowOf("Done (2024)")).toBe(done);
    expect(view.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "60");
  });

  it("marks a job that shows up after the first load, and none that were there from the start", async () => {
    vi.useFakeTimers();
    let jobs = [job({ id: "j_a", status: "running", progress: 10, requested_at: "2026-07-20T10:00:00Z", subject: "Film/Old (2020)/Old (2020).mp4" })];
    const api = apiFor(() => json(jobs));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    const rowOf = (name: string) => view.getByText(name).closest("li") as HTMLElement;
    expect(rowOf("Old (2020)")).not.toHaveClass("flash");

    jobs = [job({ id: "j_b", status: "queued", progress: 0, requested_at: "2026-07-20T10:05:00Z", started_at: null, finished_at: null, subject: "Film/New (2026)/New (2026).mp4" }), ...jobs];
    await vi.advanceTimersByTimeAsync(1100);

    expect(rowOf("New (2026)")).toHaveClass("flash");
    expect(rowOf("Old (2020)")).not.toHaveClass("flash");
  });

  it("marks a job whose status changed, but not one that only moved along", async () => {
    vi.useFakeTimers();
    let state = { status: "running", progress: 20 };
    const api = apiFor(() => json([job({ id: "j_a", status: state.status, progress: state.progress, requested_at: "2026-07-20T10:00:00Z", subject: "Film/Same (2020)/Same (2020).mp4" })]));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    const row = () => view.getByText("Same (2020)").closest("li") as HTMLElement;

    state = { status: "running", progress: 55 };
    await vi.advanceTimersByTimeAsync(4100);
    expect(row()).not.toHaveClass("flash");

    state = { status: "succeeded", progress: 100 };
    await vi.advanceTimersByTimeAsync(1100);
    expect(row()).toHaveClass("flash");
  });

  it("lists active jobs apart from history", async () => {
    const api = apiFor(() => json([
      job({ id: "j_3", status: "succeeded", subject: "Film/Arrival (2016)/Arrival (2016).mp4" }),
      job({ id: "j_2", status: "running", subject: "Film/Heat (1995)/Heat.mp4" }),
    ]));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);

    const active = await view.findByRole("region", { name: "Active" });
    const history = view.getByRole("region", { name: "History" });
    expect(within(active).getByText("Heat")).toBeInTheDocument();
    expect(within(history).getByText("Arrival (2016)")).toBeInTheDocument();
    expect(active.compareDocumentPosition(history) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("filters by one status and pages on with the same filter", async () => {
    const api = apiFor((path) =>
      path.includes("cursor=") ? json([job({ id: "j_old", subject: "Film/Old (1999)/Old (1999).mp4" })])
        : json([job({ id: "j_new", status: "failed" })], 200, { next_cursor: "older page" }));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await view.findByText("Heat");

    await fireEvent.click(view.getByRole("button", { name: "Failed" }));
    await waitFor(() => expect(calls(api).at(-1)![0]).toBe("/jobs?limit=50&order=desc&status=failed"));
    expect(view.getByRole("button", { name: "Failed" })).toHaveAttribute("aria-pressed", "true");

    await fireEvent.click(await view.findByRole("button", { name: "Load more jobs" }));
    await waitFor(() => expect(calls(api).at(-1)![0]).toBe("/jobs?limit=50&order=desc&status=failed&cursor=older%20page"));
    expect(await view.findByText("Old (1999)")).toBeInTheDocument();
    expect(view.getByText("Heat")).toBeInTheDocument();
  });

  it("keeps cancelled jobs out of All and shows them under their own filter", async () => {
    const api = apiFor(() => json([job()]));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await view.findByText("Heat");

    await fireEvent.click(view.getByRole("button", { name: "Cancelled" }));
    await waitFor(() => expect(calls(api).at(-1)![0]).toBe("/jobs?limit=50&order=desc&status=cancelled"));
  });

  it("opens a job when it is chosen", async () => {
    const onOpen = vi.fn();
    const view = render(() => <ActivityPage api={apiFor(() => json([job()]))} onOpen={onOpen} />);

    await fireEvent.click(await view.findByRole("link", { name: /Heat/ }));

    expect(onOpen).toHaveBeenCalledWith("j_1");
  });

  it("shows the open job in the detail pane and a prompt when none is chosen", async () => {
    const api = apiFor((path) => path === "/jobs/j_1" ? json(job()) : path.startsWith("/jobs/j_1/items") ? json([item()]) : json([job()]));
    const none = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    expect(await none.findByText(/select a job to see what it did/i)).toBeInTheDocument();
    none.unmount();

    const open = render(() => <ActivityPage api={api} activeId="j_1" onOpen={vi.fn()} />);
    expect(await open.findByRole("heading", { name: "Heat" })).toBeInTheDocument();
  });

  it("keeps a running job fresh by reloading while something is active, and stops when nothing is", async () => {
    vi.useFakeTimers();
    let status = "running";
    const api = apiFor(() => json([job({ status })]));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    expect(within(view.getByRole("region", { name: "Active" })).getByText("Running")).toBeInTheDocument();
    const initial = calls(api).length;

    status = "succeeded";
    await vi.advanceTimersByTimeAsync(4100);
    expect(calls(api).length).toBe(initial + 1);
    expect(within(view.getByRole("region", { name: "History" })).getByText("Succeeded")).toBeInTheDocument();

    await vi.advanceTimersByTimeAsync(12000);
    expect(calls(api).length).toBe(initial + 1);
  });

  it("does not repeat jobs when the server issues fresh ids on every response", async () => {
    vi.useFakeTimers();
    let serial = 0;
    const api = apiFor(() => {
      serial += 1;
      return json([
        job({ id: `j_a${serial}`, status: "queued", subject: "Film/Heat (1995)/Heat.mp4" }),
        job({ id: `j_b${serial}`, status: "queued", subject: "Film/Arrival (2016)/Arrival (2016).mp4", requested_at: "2026-07-20T09:00:00Z" }),
      ]);
    });
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    expect(view.getAllByRole("link")).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(4100);
    await vi.advanceTimersByTimeAsync(4100);

    expect(calls(api).length).toBeGreaterThan(2);
    expect(view.getAllByRole("link")).toHaveLength(2);
  });

  it("keeps the open job highlighted after a refresh renames every id", async () => {
    vi.useFakeTimers();
    let serial = 0;
    const api = apiFor((path) => {
      if (path.startsWith("/jobs/") ) return path.includes("/items") ? json([item()]) : json(job({ id: "j_a1", status: "queued" }));
      serial += 1;
      return json([job({ id: `j_a${serial}`, status: "queued" })]);
    });
    const view = render(() => <ActivityPage api={api} activeId="j_a1" onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    expect(view.getByRole("link", { name: /heat/i }).closest("li")).toHaveAttribute("aria-current", "true");

    await vi.advanceTimersByTimeAsync(4100);

    expect(view.getByRole("link", { name: /heat/i }).closest("li")).toHaveAttribute("aria-current", "true");
  });

  it("reloads every page it has loaded, not only the newest", async () => {
    vi.useFakeTimers();
    let serial = 0;
    const api = apiFor((path) => {
      serial += 1;
      return path.includes("cursor=")
        ? json([job({ id: `j_old${serial}`, status: "succeeded", subject: "Film/Old (1999)/Old (1999).mp4" })])
        : json([job({ id: `j_new${serial}`, status: "queued" })], 200, { next_cursor: "page 2" });
    });
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    await fireEvent.click(view.getByRole("button", { name: "Load more jobs" }));
    await vi.advanceTimersByTimeAsync(0);
    expect(view.getAllByRole("link")).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(4100);

    expect(view.getAllByRole("link")).toHaveLength(2);
    expect(view.getByText("Old (1999)")).toBeInTheDocument();
  });

  it("explains an empty history, offers a retry after an error and reports an expired session", async () => {
    const empty = render(() => <ActivityPage api={apiFor(() => json([]))} onOpen={vi.fn()} />);
    expect(await empty.findByText(/no jobs yet/i)).toBeInTheDocument();
    empty.unmount();

    let attempts = 0;
    const flaky = render(() => <ActivityPage api={apiFor(() => (++attempts === 1 ? json({}, 503) : json([job()])))} onOpen={vi.fn()} />);
    expect(await flaky.findByRole("alert")).toHaveTextContent("Unable to load jobs");
    await fireEvent.click(flaky.getByRole("button", { name: "Retry" }));
    expect(await flaky.findByText("Heat")).toBeInTheDocument();
    flaky.unmount();

    const expired = render(() => <ActivityPage api={apiFor(() => Promise.reject(new SessionError()))} onOpen={vi.fn()} />);
    expect(await expired.findByRole("alert")).toHaveTextContent(/session expired.*lock.*unlock/i);
  });
});

function detailApi(jobOver: Record<string, unknown> = {}, items: unknown[] = [item()], extra: (path: string, init?: RequestInit) => Response | Promise<Response> | undefined = () => undefined) {
  return apiFor((path, init) => {
    const special = extra(path, init);
    if (special) return special;
    if (path === "/jobs/j_1") return json(job(jobOver));
    if (path.startsWith("/jobs/j_1/items")) return json(items);
    if (path.startsWith("/job-items/")) return json({ item_id: "i_1", status: "ok", log_tail: "log line" });
    return json({});
  });
}

describe("JobDetail: what happened", () => {
  it("states the outcome, the file, the action and how long it took", async () => {
    const view = render(() => <JobDetail api={detailApi()} jobId="j_1" />);

    expect(await view.findByRole("heading", { name: "Heat" })).toBeInTheDocument();
    const summary = within(view.getByRole("group", { name: "Job summary" }));
    expect(summary.getByText("Succeeded")).toBeInTheDocument();
    expect(summary.getByText("1 of 1 succeeded")).toBeInTheDocument();
    expect(view.getByText("1 min")).toBeInTheDocument();
  });

  it("answers why a job failed with the file, the error code and the reason", async () => {
    const failed = item({ status: "failed", error_code: "synthetic_failure", error_detail: "the file changed while it was being rewritten" });
    const view = render(() => <JobDetail api={detailApi({ status: "failed", succeeded_count: 0, failed_count: 1 }, [failed])} jobId="j_1" />);

    const why = await view.findByRole("region", { name: "Why it failed" });
    expect(why).toHaveTextContent("Heat");
    expect(why).toHaveTextContent("synthetic_failure");
    expect(why).toHaveTextContent("the file changed while it was being rewritten");
  });

  it("says on the item that encoding was not allowed, as a hint and not as a failure", async () => {
    const refused = item({ error_code: "encode_not_allowed", error_detail: "Encoding not allowed (need=video allowance=audio): the file stays as it was." });
    const view = render(() => <JobDetail api={detailApi({}, [refused])} jobId="j_1" />);

    const note = await view.findByRole("status", { name: "Encoding not allowed" });
    expect(note).toHaveTextContent("Encoding not allowed");
    expect(note).toHaveTextContent("the file stays as it was");
    expect(view.queryByRole("region", { name: "Why it failed" })).not.toBeInTheDocument();
  });

  it("shows no encoding note on an item that ran normally", async () => {
    const view = render(() => <JobDetail api={detailApi()} jobId="j_1" />);
    await view.findByRole("heading", { name: "Heat" });
    expect(view.queryByRole("status", { name: "Encoding not allowed" })).not.toBeInTheDocument();
  });

  it("does not show a failure section for a job that did not fail", async () => {
    const view = render(() => <JobDetail api={detailApi()} jobId="j_1" />);
    await view.findByRole("heading", { name: "Heat" });
    expect(view.queryByRole("region", { name: "Why it failed" })).not.toBeInTheDocument();
  });

  it("words item states and numbers the items", async () => {
    const view = render(() => <JobDetail api={detailApi({ total_count: 2 }, [item({ status: "stale_input" }), item({ id: "i_2", position: 2, display_path: "Film/Arrival (2016)/Arrival (2016).mp4" })])} jobId="j_1" />);

    expect(await view.findByText("File changed")).toBeInTheDocument();
    expect(view.getByText("#1")).toBeInTheDocument();
    expect(view.getByText("#2")).toBeInTheDocument();
    expect(view.getByText("Arrival (2016)")).toBeInTheDocument();
  });

  it("loads more items on request without duplicating the ones already shown", async () => {
    const second = item({ id: "i_second", position: 2, display_path: "Film/Arrival (2016)/Arrival (2016).mp4" });
    const api = detailApi({ total_count: 2 }, [item()], (path) =>
      path.includes("cursor=") ? json([item(), second])
        : path.startsWith("/jobs/j_1/items") ? json([item()], 200, { next_cursor: "item next" }) : undefined);
    const view = render(() => <JobDetail api={api} jobId="j_1" />);

    await fireEvent.click(await view.findByRole("button", { name: /load more items/i }));

    await waitFor(() => expect(calls(api).at(-1)![0]).toBe("/jobs/j_1/items?limit=50&cursor=item%20next"));
    await waitFor(() => expect(view.getByText("#2")).toBeInTheDocument());
    expect(view.getAllByText("#1")).toHaveLength(1);
  });

  it("retries failed item and job loads independently", async () => {
    let itemAttempts = 0;
    const api = detailApi({}, [item()], (path) => (path.startsWith("/jobs/j_1/items") && ++itemAttempts === 1 ? json([], 500) : undefined));
    const view = render(() => <JobDetail api={api} jobId="j_1" />);

    await fireEvent.click(await view.findByRole("button", { name: /retry loading items/i }));

    expect(await view.findByText("#1")).toBeInTheDocument();
  });

  it("shows a generic session message and a retryable error", async () => {
    const expired = render(() => <JobDetail api={apiFor(() => Promise.reject(new SessionError()))} jobId="j_1" />);
    expect((await expired.findAllByRole("alert")).some((alert) => /session expired.*lock.*unlock/i.test(alert.textContent ?? ""))).toBe(true);
    expired.unmount();

    let failed = true;
    const flaky = render(() => <JobDetail api={detailApi({}, [item()], (path) => (path === "/jobs/j_1" && failed ? ((failed = false), json({}, 503)) : undefined))} jobId="j_1" />);
    expect(await flaky.findByRole("alert")).toHaveTextContent("Unable to load job");
    await fireEvent.click(flaky.getByRole("button", { name: "Retry" }));
    expect(await flaky.findByRole("heading", { name: "Heat" })).toBeInTheDocument();
  });

  it("loads a log only when it is opened", async () => {
    const api = detailApi();
    const view = render(() => <JobDetail api={api} jobId="j_1" />);
    await view.findByText("#1");
    expect(calls(api).some(([path]) => path.startsWith("/job-items/"))).toBe(false);

    await fireEvent.click(view.getByText("Log"));

    expect(await view.findByLabelText("Plain text log")).toHaveTextContent("log line");
    expect(calls(api).some(([path]) => path === "/job-items/i_1/log")).toBe(true);
  });

  it("opens the log of the entry in the job on screen, not of the same-looking entry of the job before", async () => {
    const { createSignal } = await import("solid-js");
    const api = apiFor((path) => {
      if (path === "/jobs/j_a") return json(job({ id: "j_a" }));
      if (path === "/jobs/j_b") return json(job({ id: "j_b" }));
      if (path.startsWith("/jobs/j_a/items")) return json([item({ id: "i_a0", job_id: "j_a" })]);
      if (path.startsWith("/jobs/j_b/items")) return json([item({ id: "i_b0", job_id: "j_b" })]);
      if (path.startsWith("/job-items/")) return json({ item_id: "x", status: "ok", log_tail: "log" });
      return json({});
    });
    const [jobId, setJobId] = createSignal("j_a");
    const view = render(() => <JobDetail api={api} jobId={jobId()} />);
    await view.findByText("#1");
    await fireEvent.click(view.getByText("Log"));
    await waitFor(() => expect(calls(api).some(([path]) => path === "/job-items/i_a0/log")).toBe(true));

    setJobId("j_b");
    await waitFor(() => expect(calls(api).some(([path]) => path.startsWith("/jobs/j_b/items"))).toBe(true));
    await view.findByText("#1");
    await fireEvent.click(await view.findByText("Log"));

    await waitFor(() => expect(calls(api).map(([path]) => path)).toContain("/job-items/i_b0/log"));
    expect(await view.findAllByLabelText("Plain text log")).toHaveLength(1);
  });

  it("keeps an active job fresh by reloading it until it ends", async () => {
    vi.useFakeTimers();
    let status = "running";
    const api = apiFor((path) => path === "/jobs/j_1" ? json(job({ status })) : json([item({ status: "running" })]));
    const view = render(() => <JobDetail api={api} jobId="j_1" />);
    await vi.advanceTimersByTimeAsync(0);
    const summary = () => within(view.getByRole("group", { name: "Job summary" }));
    expect(summary().getByText("Running")).toBeInTheDocument();

    status = "succeeded";
    await vi.advanceTimersByTimeAsync(4100);

    expect(summary().getByText("Succeeded")).toBeInTheDocument();
  });
});

describe("JobDetail: a running job", () => {
  it("shows overall and per-item progress and how long it has been running", async () => {
    const started = new Date(Date.now() - 75_000).toISOString();
    const items = [
      item({ id: "i_1", position: 0, status: "succeeded", progress: 100 }),
      item({ id: "i_2", position: 1, status: "running", progress: 42, started_at: started }),
      item({ id: "i_3", position: 2, status: "queued", progress: null }),
    ];
    const view = render(() => <JobDetail api={detailApi({ status: "running", total_count: 3, succeeded_count: 1, progress: 47, started_at: started, finished_at: null }, items)} jobId="j_1" />);

    const overall = await view.findByRole("progressbar", { name: "Job progress" });

    expect(overall).toHaveAttribute("aria-valuenow", "47");
    expect(view.getByRole("group", { name: "Job summary" })).toHaveTextContent("47%");
    expect(view.getByText("Running for").nextElementSibling).toHaveTextContent(/1 min/);
    const running = await view.findByRole("progressbar", { name: /item 1 progress/i });
    expect(running).toHaveAttribute("aria-valuenow", "42");
    expect(view.getAllByRole("progressbar")).toHaveLength(2);
  });

  it("shows no progress for a finished job", async () => {
    const view = render(() => <JobDetail api={detailApi({ status: "succeeded", progress: 100 }, [item({ progress: 100 })])} jobId="j_1" />);
    await view.findByRole("heading", { name: "Heat" });

    expect(view.queryByRole("progressbar")).not.toBeInTheDocument();
    expect(view.queryByText("Running for")).not.toBeInTheDocument();
  });
});

describe("JobDetail: refreshing a running job", () => {
  /** Fresh ids on every response, like the real server; only the item's progress ever changes. */
  function liveApi(state: { progress: number }) {
    let serial = 0;
    return apiFor((path) => {
      serial += 1;
      const started = "2026-07-20T10:00:00Z";
      if (path === "/jobs/j_1") return json(job({ id: `j_${serial}`, status: "running", total_count: 2, succeeded_count: 1, progress: 50 + state.progress / 2, started_at: started, finished_at: null }));
      if (path.startsWith("/jobs/j_1/items")) {
        return json([
          item({ id: `i_a${serial}`, position: 0, status: "succeeded", progress: 100 }),
          item({ id: `i_b${serial}`, position: 1, status: "running", progress: state.progress, started_at: started }),
        ]);
      }
      if (path.startsWith("/job-items/")) return json({ item_id: "i", status: "ok", log_tail: "log line" });
      return json({});
    });
  }

  it("leaves the page alone when nothing changed, whatever ids the server hands out", async () => {
    vi.useFakeTimers();
    const view = render(() => <JobDetail api={liveApi({ progress: 40 })} jobId="j_1" />);
    await vi.advanceTimersByTimeAsync(0);
    const cancel = view.getByRole("button", { name: "Cancel job" });
    const bars = view.getAllByRole("progressbar");
    const logToggles = view.getAllByRole("button", { name: "Log" });
    await fireEvent.click(logToggles[1]!);
    await vi.advanceTimersByTimeAsync(0);
    const log = view.getByText("log line");

    await vi.advanceTimersByTimeAsync(13000);

    expect(view.getByRole("button", { name: "Cancel job" })).toBe(cancel);
    expect(view.getAllByRole("progressbar")).toEqual(bars);
    expect(view.getByText("log line")).toBe(log);
    expect(view.queryByText(/loading/i)).not.toBeInTheDocument();
  });

  it("updates just the number that moved and keeps the open log and the buttons", async () => {
    vi.useFakeTimers();
    const state = { progress: 40 };
    const view = render(() => <JobDetail api={liveApi(state)} jobId="j_1" />);
    await vi.advanceTimersByTimeAsync(0);
    const cancel = view.getByRole("button", { name: "Cancel job" });
    const itemBar = view.getByRole("progressbar", { name: /item 1 progress/i });
    await fireEvent.click(view.getAllByRole("button", { name: "Log" })[1]!);
    await vi.advanceTimersByTimeAsync(0);
    const log = view.getByText("log line");

    state.progress = 70;
    await vi.advanceTimersByTimeAsync(4100);

    expect(view.getByRole("progressbar", { name: /item 1 progress/i })).toBe(itemBar);
    expect(itemBar).toHaveAttribute("aria-valuenow", "70");
    expect(view.getByRole("button", { name: "Cancel job" })).toBe(cancel);
    expect(view.getByText("log line")).toBe(log);
  });
});

describe("LogViewer: following a running log", () => {
  const metrics = (scrollHeight: number, clientHeight: number) => {
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get: () => scrollHeight });
    Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => clientHeight });
  };
  afterEach(() => {
    delete (HTMLElement.prototype as unknown as Record<string, unknown>).scrollHeight;
    delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientHeight;
  });
  const growingLog = () => {
    let text = "first line\n";
    const api = apiFor(() => json({ item_id: "i_1", status: "running", log_tail: text }));
    return { api, grow: (more: string) => { text += more; } };
  };

  it("keeps the newest lines in view while the reader has not scrolled away", async () => {
    vi.useFakeTimers();
    metrics(1000, 320);
    const { api, grow } = growingLog();
    const view = render(() => <LogViewer api={api} itemId="i_1" />);
    await vi.advanceTimersByTimeAsync(0);
    const pre = view.getByLabelText("Plain text log") as HTMLElement;
    expect(pre.scrollTop).toBe(1000);

    grow("second line\n");
    metrics(1200, 320);
    await vi.advanceTimersByTimeAsync(3100);

    expect(pre).toHaveTextContent("second line");
    expect(pre.scrollTop).toBe(1200);
  });

  it("leaves the reader where they are once they have scrolled up", async () => {
    vi.useFakeTimers();
    metrics(1000, 320);
    const { api, grow } = growingLog();
    const view = render(() => <LogViewer api={api} itemId="i_1" />);
    await vi.advanceTimersByTimeAsync(0);
    const pre = view.getByLabelText("Plain text log") as HTMLElement;
    pre.scrollTop = 100;
    pre.dispatchEvent(new Event("scroll"));

    grow("a later line\n");
    metrics(1200, 320);
    await vi.advanceTimersByTimeAsync(3100);

    expect(pre).toHaveTextContent("a later line");
    expect(pre.scrollTop).toBe(100);
  });
});

describe("JobDetail: only the actions the server will accept", () => {
  const buttons = async (status: string) => {
    const view = render(() => <JobDetail api={detailApi({ status })} jobId="j_1" />);
    await view.findByRole("heading", { name: "Heat" });
    const result = { cancel: !!view.queryByRole("button", { name: "Cancel job" }), retry: !!view.queryByRole("button", { name: "Retry failed items" }) };
    view.unmount();
    return result;
  };

  it("offers Cancel while a job can be stopped, Retry when it left failures, and neither otherwise", async () => {
    expect(await buttons("running")).toEqual({ cancel: true, retry: false });
    expect(await buttons("queued")).toEqual({ cancel: true, retry: false });
    expect(await buttons("failed")).toEqual({ cancel: false, retry: true });
    expect(await buttons("partial_success")).toEqual({ cancel: false, retry: true });
    expect(await buttons("succeeded")).toEqual({ cancel: false, retry: false });
    expect(await buttons("cancelled")).toEqual({ cancel: false, retry: false });
  });
});

describe("JobDetail: cancel and retry", () => {
  const posts = (api: Api, path: string) => calls(api).filter(([p, init]) => p === path && init?.method === "POST");

  it("confirms inline, never in a modal, and sends nothing before the confirmation", async () => {
    const api = detailApi({ status: "running" });
    const view = render(() => <JobDetail api={api} jobId="j_1" />);

    await fireEvent.click(await view.findByRole("button", { name: "Cancel job" }));

    expect(view.queryByRole("dialog")).not.toBeInTheDocument();
    expect(view.getByRole("region", { name: "Confirm cancel" })).toBeInTheDocument();
    expect(posts(api, "/jobs/j_1/cancel")).toHaveLength(0);
  });

  it("blocks a second submit while the first is in flight and says what was requested", async () => {
    let finish!: (value: Response) => void;
    const onChanged = vi.fn();
    const api = detailApi({ status: "running" }, [item()], (path) => (path === "/jobs/j_1/cancel" ? new Promise<Response>((resolve) => { finish = resolve; }) : undefined));
    const view = render(() => <JobDetail api={api} jobId="j_1" onChanged={onChanged} />);
    await fireEvent.click(await view.findByRole("button", { name: "Cancel job" }));

    const confirm = view.getByRole("button", { name: "Confirm cancel" });
    await fireEvent.click(confirm);
    expect(confirm).toBeDisabled();
    await fireEvent.click(confirm);
    expect(posts(api, "/jobs/j_1/cancel")).toHaveLength(1);

    finish(json({ status: "cancel_requested" }));
    expect(await view.findByText("Job cancellation requested")).toBeInTheDocument();
    expect(onChanged).toHaveBeenCalled();
  });

  it("reuses one idempotency key when the same confirmation is retried", async () => {
    let attempts = 0;
    const api = detailApi({ status: "running" }, [item()], (path) => (path === "/jobs/j_1/cancel" ? (++attempts === 1 ? json({}, 503) : json({ status: "cancel_requested" })) : undefined));
    const view = render(() => <JobDetail api={api} jobId="j_1" />);
    await fireEvent.click(await view.findByRole("button", { name: "Cancel job" }));

    await fireEvent.click(view.getByRole("button", { name: "Confirm cancel" }));
    expect(await view.findByRole("alert")).toHaveTextContent(/unable to update job/i);
    await fireEvent.click(view.getByRole("button", { name: "Confirm cancel" }));
    await view.findByText("Job cancellation requested");

    const [first, second] = posts(api, "/jobs/j_1/cancel");
    const key = new Headers(first![1]?.headers).get("Idempotency-Key");
    expect(key).toBeTruthy();
    expect(new Headers(second![1]?.headers).get("Idempotency-Key")).toBe(key);
  });

  it("moves focus to Confirm, closes on Escape and hands focus back to the trigger", async () => {
    const view = render(() => <JobDetail api={detailApi({ status: "running" })} jobId="j_1" />);
    const trigger = await view.findByRole("button", { name: "Cancel job" });
    trigger.focus();
    await fireEvent.click(trigger);

    const confirm = view.getByRole("button", { name: "Confirm cancel" });
    await waitFor(() => expect(document.activeElement).toBe(confirm));
    await fireEvent.keyDown(view.getByRole("region", { name: "Confirm cancel" }), { key: "Escape" });

    await waitFor(() => expect(view.queryByRole("region", { name: "Confirm cancel" })).not.toBeInTheDocument());
    expect(document.activeElement).toBe(trigger);
  });

  it("dismisses the confirmation without sending anything", async () => {
    const api = detailApi({ status: "running" });
    const view = render(() => <JobDetail api={api} jobId="j_1" />);
    await fireEvent.click(await view.findByRole("button", { name: "Cancel job" }));

    await fireEvent.click(view.getByRole("button", { name: "Never mind" }));

    expect(view.queryByRole("region", { name: "Confirm cancel" })).not.toBeInTheDocument();
    expect(posts(api, "/jobs/j_1/cancel")).toHaveLength(0);
  });

  it("retries only the failed items, reports it and hands over the new job", async () => {
    const onRetried = vi.fn();
    const api = detailApi({ status: "partial_success" }, [item({ status: "failed" })], (path) => (path === "/jobs/j_1/retry" ? json({ id: "j_new", status: "queued" }) : undefined));
    const view = render(() => <JobDetail api={api} jobId="j_1" onRetried={onRetried} />);

    await fireEvent.click(await view.findByRole("button", { name: "Retry failed items" }));
    await fireEvent.click(view.getByRole("button", { name: "Confirm retry" }));

    expect(await view.findByText("Job retry requested")).toBeInTheDocument();
    expect(onRetried).toHaveBeenCalledWith("j_new");
  });

  it("closes the confirmation and reports an expired session", async () => {
    const api = detailApi({ status: "partial_success" }, [item({ status: "failed" })], (path) => (path === "/jobs/j_1/retry" ? Promise.reject(new SessionError()) : undefined));
    const view = render(() => <JobDetail api={api} jobId="j_1" />);
    const retry = await view.findByRole("button", { name: "Retry failed items" });
    await fireEvent.click(retry);
    await fireEvent.click(view.getByRole("button", { name: "Confirm retry" }));

    await expect(view.findByRole("alert")).resolves.toHaveTextContent(/session expired/i);
    expect(view.queryByRole("region", { name: "Confirm retry" })).not.toBeInTheDocument();
    await waitFor(() => expect(document.activeElement).toBe(retry));
  });
});

describe("LogViewer", () => {
  it("renders a generic session state and downloads only plain text", async () => {
    const expired = render(() => <LogViewer api={apiFor(() => Promise.reject(new SessionError()))} itemId="i_1" />);
    expect(await expired.findByRole("alert")).toHaveTextContent(/session expired/i);
    expired.unmount();

    if (!URL.createObjectURL) URL.createObjectURL = () => "blob:test";
    const createObjectURL = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:test");
    const view = render(() => <LogViewer api={apiFor(() => json({ item_id: "i_1", status: "ok", log_tail: "hello" }))} itemId="i_1" />);
    await view.findByText("hello");
    await fireEvent.click(view.getByRole("button", { name: "Download log" }));

    expect(createObjectURL).toHaveBeenCalledWith(expect.any(Blob));
    expect((createObjectURL.mock.calls[0]?.[0] as Blob).type).toBe("text/plain");
    createObjectURL.mockRestore();
  });

  it("keeps log text unparsed", async () => {
    const view = render(() => <LogViewer api={apiFor(() => json({ item_id: "i_1", status: "ok", log_tail: "<b>plain log</b>\nnext" }))} itemId="i_1" />);

    expect((await view.findByLabelText("Plain text log")).textContent).toContain("<b>plain log</b>");
    expect(view.container.querySelector("b")).not.toBeInTheDocument();
  });

  it("revokes the download URL after the download opportunity", async () => {
    vi.useFakeTimers();
    Object.defineProperty(URL, "revokeObjectURL", { configurable: true, writable: true, value: vi.fn() });
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    try {
      const view = render(() => <LogViewer api={apiFor(() => json({ item_id: "i_1", status: "ok", log_tail: "hello" }))} itemId="i_1" />);
      await vi.advanceTimersByTimeAsync(0);
      const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:test");
      await fireEvent.click(view.getByRole("button", { name: "Download log" }));
      expect(revoke).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(0);
      expect(revoke).toHaveBeenCalledWith("blob:test");
      create.mockRestore();
    } finally {
      revoke.mockRestore();
      Reflect.deleteProperty(URL, "revokeObjectURL");
    }
  });

  it("stops polling while paused and resumes on request", async () => {
    vi.useFakeTimers();
    const api = apiFor(() => json({ item_id: "i_1", status: "ok", log_tail: "hello" }));
    const view = render(() => <LogViewer api={api} itemId="i_1" />);
    await vi.advanceTimersByTimeAsync(0);
    expect(calls(api)).toHaveLength(1);

    await fireEvent.click(view.getByRole("button", { name: "Pause log" }));
    await vi.advanceTimersByTimeAsync(6000);
    expect(calls(api)).toHaveLength(1);

    await fireEvent.click(view.getByRole("button", { name: "Resume log" }));
    await vi.advanceTimersByTimeAsync(3100);
    expect(calls(api)).toHaveLength(2);
  });

  it("offers an independent retry when the log cannot be loaded", async () => {
    let attempts = 0;
    const view = render(() => <LogViewer api={apiFor(() => (++attempts === 1 ? json({}, 500) : json({ item_id: "i_1", status: "ok", log_tail: "retried" })))} itemId="i_1" />);

    await fireEvent.click(await view.findByRole("button", { name: /retry loading log/i }));

    expect(await view.findByText("retried")).toBeInTheDocument();
  });
});

describe("ActivityPage: clearing finished jobs", () => {
  const clearApi = (cleared = 3, respond?: (path: string) => Response | undefined) => apiFor((path, init) => {
    const custom = respond?.(path);
    if (custom) return custom;
    if (path === "/jobs/clear" && init?.method === "POST") return json({ cleared });
    return path.startsWith("/jobs?") ? json([job()]) : json([]);
  });
  const posts = (api: Api) => calls(api).filter(([p, init]) => p === "/jobs/clear" && init?.method === "POST");

  it("asks inline which results to clear and sends nothing before the confirmation", async () => {
    const api = clearApi();
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await view.findByText("Heat");

    await fireEvent.click(view.getByRole("button", { name: "Clear finished" }));

    expect(view.queryByRole("dialog")).not.toBeInTheDocument();
    const panel = view.getByRole("region", { name: "Clear finished jobs" });
    expect(within(panel).getByLabelText("Succeeded")).toBeChecked();
    expect(within(panel).getByLabelText("Failed")).toBeChecked();
    expect(within(panel).getByLabelText("Cancelled")).not.toBeChecked();
    expect(posts(api)).toHaveLength(0);
  });

  it("clears the chosen results, says how many, and reloads the list", async () => {
    const api = clearApi(3);
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await view.findByText("Heat");
    await fireEvent.click(view.getByRole("button", { name: "Clear finished" }));
    const panel = view.getByRole("region", { name: "Clear finished jobs" });
    await fireEvent.click(within(panel).getByLabelText("Cancelled"));
    await fireEvent.click(within(panel).getByLabelText("Succeeded"));

    await fireEvent.click(within(panel).getByRole("button", { name: "Confirm clear" }));

    expect(await view.findByText("Cleared 3 jobs. They stay in the audit trail.")).toBeInTheDocument();
    expect(JSON.parse(String(posts(api)[0]![1]!.body))).toEqual({ statuses: ["failed", "partial_success", "cancelled"] });
    expect(view.queryByRole("region", { name: "Clear finished jobs" })).not.toBeInTheDocument();
    expect(calls(api).filter(([p]) => p.startsWith("/jobs?")).length).toBeGreaterThan(1);
  });

  it("will not clear with nothing selected and can be dismissed with Escape", async () => {
    const api = clearApi();
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await view.findByText("Heat");
    await fireEvent.click(view.getByRole("button", { name: "Clear finished" }));
    const panel = view.getByRole("region", { name: "Clear finished jobs" });
    await fireEvent.click(within(panel).getByLabelText("Succeeded"));
    await fireEvent.click(within(panel).getByLabelText("Failed"));

    expect(within(panel).getByRole("button", { name: "Confirm clear" })).toBeDisabled();
    await fireEvent.keyDown(panel, { key: "Escape" });
    expect(view.queryByRole("region", { name: "Clear finished jobs" })).not.toBeInTheDocument();
    expect(posts(api)).toHaveLength(0);
  });

  it("reuses one idempotency key when the same confirmation is retried after an error", async () => {
    let attempts = 0;
    const api = clearApi(2, (path) => (path === "/jobs/clear" && ++attempts === 1 ? json({}, 503) : undefined));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await view.findByText("Heat");
    await fireEvent.click(view.getByRole("button", { name: "Clear finished" }));
    const panel = view.getByRole("region", { name: "Clear finished jobs" });

    await fireEvent.click(within(panel).getByRole("button", { name: "Confirm clear" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent(/unable to clear/i);
    await fireEvent.click(within(panel).getByRole("button", { name: "Confirm clear" }));
    await view.findByText(/Cleared 2 jobs/);

    const [first, second] = posts(api);
    const key = new Headers(first![1]?.headers).get("Idempotency-Key");
    expect(key).toBeTruthy();
    expect(new Headers(second![1]?.headers).get("Idempotency-Key")).toBe(key);
  });
});

describe("ActivityPage: a running job, live", () => {
  it("counts how long it has been running every second, with or without a known percent", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-20T10:00:10Z"));
    const api = apiFor(() => json([
      job({ id: "j_a", status: "running", progress: 40, requested_at: "2026-07-20T10:00:00Z", started_at: "2026-07-20T10:00:00Z", finished_at: null, subject: "Film/Alpha (2025)/Alpha (2025).mp4" }),
      job({ id: "j_b", status: "running", progress: null, requested_at: "2026-07-20T10:00:01Z", started_at: "2026-07-20T10:00:05Z", finished_at: null, subject: "Film/Beta (2025)/Beta (2025).mp4" }),
    ]));
    const view = render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    const rowOf = (name: string) => view.getByText(name).closest("li") as HTMLElement;

    expect(rowOf("Alpha (2025)")).toHaveTextContent("40% · running for 10 s");
    expect(rowOf("Beta (2025)")).toHaveTextContent("running for 5 s");
    expect(within(rowOf("Beta (2025)")).queryByRole("progressbar")).toBeNull();

    await vi.advanceTimersByTimeAsync(1000);

    expect(rowOf("Alpha (2025)")).toHaveTextContent("running for 11 s");
    expect(rowOf("Beta (2025)")).toHaveTextContent("running for 6 s");
  });

  it("asks the server for a fresh state once a second while something is active", async () => {
    vi.useFakeTimers();
    const api = apiFor(() => json([job({ id: "j_a", status: "running", progress: 5, finished_at: null })]));
    render(() => <ActivityPage api={api} onOpen={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    const before = calls(api).length;

    await vi.advanceTimersByTimeAsync(3000);

    expect(calls(api).length - before).toBe(3);
  });
});

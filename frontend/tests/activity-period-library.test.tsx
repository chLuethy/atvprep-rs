import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { ActivityPage } from "../src/features/activity/ActivityPage";
import { EMPTY_JOB_FILTER } from "../src/features/activity/filters";
import { dayOf } from "../src/features/activity/period";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };
const json = (data: unknown) => new Response(JSON.stringify({ data, meta: {} }), { status: 200 });
const COUNTS = {
  status: { queued: 0, running: 0, cancel_requested: 0, succeeded: 3, partial_success: 0, failed: 1, cancelled: 0 },
  action: { verify: 4, prepare: 0, inspect: 0 },
  root_id: { r_movies: 3, r_shows: 1, r_empty: 0 },
  matched: 4, total: 4, hidden_active: 0,
};
const ROOTS = [
  { id: "r_movies", name: "Movies", latest_scan: null },
  { id: "r_shows", name: "Shows", latest_scan: null },
  { id: "r_empty", name: "Empty", latest_scan: null },
];
const job = { id: "j_1", action: "verify", status: "succeeded", trigger: "manual", origin_job_id: null, requested_at: "2026-11-14T08:00:00Z", started_at: null, finished_at: null, total_count: 1, succeeded_count: 1, failed_count: 0, cancelled_count: 0, subject: "Heat.mkv" };

function setup(filter = EMPTY_JOB_FILTER) {
  const request = vi.fn(async (path: string) => {
    if (path === "/roots") return json(ROOTS);
    if (path.startsWith("/jobs/counts?")) return json(COUNTS);
    if (path.startsWith("/jobs?")) return json([job]);
    return json([]);
  });
  const api: Api = { request };
  const onFilter = vi.fn();
  const view = render(() => <ActivityPage api={api} filter={filter} onFilter={onFilter} onOpen={vi.fn()} />);
  const paths = () => request.mock.calls.map((call) => call[0] as string);
  const lastList = () => paths().filter((path) => path.startsWith("/jobs?")).at(-1) ?? "";
  const lastCounts = () => paths().filter((path) => path.startsWith("/jobs/counts?")).at(-1) ?? "";
  return { view, onFilter, lastList, lastCounts };
}

const bar = async (view: ReturnType<typeof render>) => within(await view.findByRole("search", { name: "Filter jobs" }));
const openFacet = async (view: ReturnType<typeof render>, title: string) => {
  await fireEvent.click((await bar(view)).getByRole("button", { name: new RegExp(`^${title}(: .+)?$`) }));
  return within(view.getByRole("dialog", { name: title }));
};

describe("ActivityPage: period and library under More filters", () => {
  it("keeps both under More filters, which stays open while a period is chosen", async () => {
    const { view, onFilter, lastList } = setup();
    const filters = await bar(view);
    expect(filters.queryByRole("button", { name: "Period" })).not.toBeInTheDocument();
    await fireEvent.click(filters.getByRole("button", { name: "More filters" }));
    const period = await openFacet(view, "Period");
    expect(period.getAllByRole("radio").map((radio) => radio.closest("label")?.textContent)).toEqual(["Today", "Last 24 hours", "Last 7 days", "Last 30 days", "Custom period"]);

    await fireEvent.click(period.getByRole("radio", { name: "Last 24 hours" }));

    expect(onFilter).toHaveBeenLastCalledWith({ ...EMPTY_JOB_FILTER, period: { preset: "24h" } }, { replace: false });
    await waitFor(() => expect(lastList()).toMatch(/&from=\d{4}-\d{2}-\d{2}T[^&]+Z$/));
    expect(filters.getByRole("button", { name: "More filters" })).toBeDisabled();
    expect(filters.getByRole("button", { name: "Period: Last 24 hours" })).toBeInTheDocument();
  });

  it("takes a custom period as fixed days, starting with today, and sends its bounds", async () => {
    const { view, onFilter, lastList } = setup();
    await fireEvent.click((await bar(view)).getByRole("button", { name: "More filters" }));
    const period = await openFacet(view, "Period");
    const today = dayOf(new Date());

    await fireEvent.click(period.getByRole("radio", { name: "Custom period" }));
    expect(onFilter).toHaveBeenLastCalledWith({ ...EMPTY_JOB_FILTER, period: { from: today, to: today } }, { replace: false });

    await fireEvent.change(period.getByLabelText("From"), { target: { value: "2026-11-01" } });
    await fireEvent.change(period.getByLabelText("To"), { target: { value: "2026-11-05" } });

    expect(onFilter).toHaveBeenLastCalledWith({ ...EMPTY_JOB_FILTER, period: { from: "2026-11-01", to: "2026-11-05" } }, { replace: false });
    await waitFor(() => expect(lastList()).toContain(`from=${encodeURIComponent(new Date(2026, 10, 1).toISOString())}&to=${encodeURIComponent(new Date(2026, 10, 6).toISOString())}`));
    expect(period.getByRole("radio", { name: /^Custom period|–/ })).toBeChecked();
  });

  it("shows the period from the address with More filters open and its days in the fields", async () => {
    const { view } = setup({ ...EMPTY_JOB_FILTER, period: { from: "2026-11-01", to: "2026-11-05" } });
    expect(await view.findByRole("group", { name: "More filters" })).toBeInTheDocument();
    const period = await openFacet(view, "Period");
    expect(period.getByLabelText("From")).toHaveValue("2026-11-01");
    expect(period.getByLabelText("To")).toHaveValue("2026-11-05");
  });

  it("offers every library by name with its count and narrows the list and the counters", async () => {
    const { view, onFilter, lastList, lastCounts } = setup();
    await fireEvent.click((await bar(view)).getByRole("button", { name: "More filters" }));
    const library = await openFacet(view, "Library");
    expect(await library.findByRole("checkbox", { name: "Movies, 3 jobs" })).toBeInTheDocument();
    expect(library.getByRole("checkbox", { name: "Empty, 0 jobs" })).toBeInTheDocument();

    await fireEvent.click(library.getByRole("checkbox", { name: "Shows, 1 job" }));

    expect(onFilter).toHaveBeenLastCalledWith({ ...EMPTY_JOB_FILTER, root: ["r_shows"] }, { replace: false });
    await waitFor(() => expect(lastList()).toContain("root_id=r_shows"));
    expect(lastCounts()).toContain("root_id=r_shows");
  });
});

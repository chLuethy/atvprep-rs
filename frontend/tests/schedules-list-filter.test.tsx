import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { SchedulesPage } from "../src/features/schedules/SchedulesPage";
import { filterSchedules, sortSchedules, type ListFilter } from "../src/features/schedules/model";
import type { Schedule } from "../src/features/schedules/loaders";

const s = (over: Partial<Schedule>): Schedule => ({
  id: "sc_1", name: "A", action: "verify", root_id: null, root_name: null, compliance_state: null,
  cron: "", days: [0], time: "03:00", max_files: 25, enabled: true, matching: null,
  next_run_at: null, last_run_at: null, last_result: null, last_job_id: null, ...over,
});
const names = (rows: Schedule[]) => rows.map((row) => row.name);
const none: ListFilter = { actions: [], statuses: [] };

describe("sortSchedules", () => {
  const rows = [
    s({ id: "1", name: "Charlie", next_run_at: "2026-11-08T03:00:00+00:00", last_run_at: "2026-11-01T03:00:00+00:00" }),
    s({ id: "2", name: "alpha", next_run_at: null, last_run_at: null }),
    s({ id: "3", name: "Bravo", next_run_at: "2026-11-07T03:00:00+00:00", last_run_at: "2026-11-05T03:00:00+00:00" }),
  ];
  it("sorts by next run, soonest first, schedules without one last", () => {
    expect(names(sortSchedules(rows, "next"))).toEqual(["Bravo", "Charlie", "alpha"]);
  });
  it("sorts by name ignoring case", () => {
    expect(names(sortSchedules(rows, "name"))).toEqual(["alpha", "Bravo", "Charlie"]);
  });
  it("sorts by last run, most recent first, never-run last", () => {
    expect(names(sortSchedules(rows, "last"))).toEqual(["Bravo", "Charlie", "alpha"]);
  });
  it("does not change the input", () => {
    const copy = [...rows];
    sortSchedules(rows, "name");
    expect(rows).toEqual(copy);
  });
});

describe("filterSchedules", () => {
  const rows = [
    s({ name: "v-on", action: "verify", enabled: true }),
    s({ name: "v-off", action: "verify", enabled: false }),
    s({ name: "sc-on", action: "scan", enabled: true }),
    s({ name: "f-off", action: "prepare", enabled: false }),
  ];
  it("keeps everything without a selection", () => {
    expect(names(filterSchedules(rows, none))).toEqual(["v-on", "v-off", "sc-on", "f-off"]);
  });
  it("filters by any selected action", () => {
    expect(names(filterSchedules(rows, { actions: ["scan", "prepare"], statuses: [] }))).toEqual(["sc-on", "f-off"]);
  });
  it("filters by status", () => {
    expect(names(filterSchedules(rows, { actions: [], statuses: ["paused"] }))).toEqual(["v-off", "f-off"]);
  });
  it("combines action and status", () => {
    expect(names(filterSchedules(rows, { actions: ["verify"], statuses: ["on"] }))).toEqual(["v-on"]);
  });
});

const json = (data: unknown) => new Response(JSON.stringify({ data, meta: {} }), { status: 200 });
const page = async (list: Schedule[]) => {
  const api = { request: vi.fn(async (path: string) => json(path === "/schedules" ? list : path === "/roots" ? [] : {})) };
  const view = render(() => <SchedulesPage api={api} />);
  await view.findByRole("list", { name: "Schedules" });
  return view;
};
const listNames = (view: ReturnType<typeof render>) =>
  within(view.getByRole("list", { name: "Schedules" })).getAllByRole("listitem").map((li) => li.querySelector(".sched-name")?.textContent);

describe("schedule list toolbar", () => {
  const rows = [
    s({ id: "1", name: "Later", next_run_at: "2026-11-09T00:00:00+00:00" }),
    s({ id: "2", name: "Sooner", next_run_at: "2026-11-07T00:00:00+00:00" }),
    s({ id: "3", name: "Scanner", action: "scan", enabled: false, next_run_at: null }),
  ];
  it("defaults to next run and re-sorts by name", async () => {
    const view = await page(rows);
    expect(listNames(view)).toEqual(["Sooner", "Later", "Scanner"]);
    fireEvent.change(view.getByLabelText("Sort by"), { target: { value: "name" } });
    expect(listNames(view)).toEqual(["Later", "Scanner", "Sooner"]);
  });
  it("filters by action chip and shows the pressed state", async () => {
    const view = await page(rows);
    const chip = view.getByRole("button", { name: "Scan" });
    expect(chip.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(chip);
    expect(chip.getAttribute("aria-pressed")).toBe("true");
    expect(listNames(view)).toEqual(["Scanner"]);
  });
  it("filters by status chip", async () => {
    const view = await page(rows);
    fireEvent.click(view.getByRole("button", { name: "Paused" }));
    expect(listNames(view)).toEqual(["Scanner"]);
  });
  it("shows an empty state with a reset when filters hide everything", async () => {
    const view = await page(rows);
    fireEvent.click(view.getByRole("button", { name: "Prepare" }));
    expect(view.getByText("No schedule matches these filters")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "Reset filters" }));
    await waitFor(() => expect(listNames(view)).toHaveLength(3));
  });
});

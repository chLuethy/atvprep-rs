import { render, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { SchedulesPage } from "../src/features/schedules/SchedulesPage";
import type { Schedule } from "../src/features/schedules/loaders";

const json = (data: unknown, meta: unknown = {}) => new Response(JSON.stringify({ data, meta }), { status: 200 });
const schedule = (over: Partial<Schedule> = {}): Schedule => ({
  id: "sc_1", name: "Nightly verify", action: "verify", root_id: "r_1", root_name: "Film", compliance_state: "unknown",
  cron: "0 3 * * 1,2,3,4,5", days: [0, 1, 2, 3, 4], time: "03:00", max_files: 25, enabled: true, matching: 12,
  next_run_at: "2026-10-26T02:00:00+00:00", last_run_at: "2026-10-25T02:00:00+00:00", last_result: "Queued 12 files", last_job_id: "j_9",
  runs: [{ started_at: "2026-10-25T02:00:00+00:00", trigger: "scheduled", result: "Queued 12 files", kind: "run", job_id: "j_9", scan_id: null }],
  ...over,
});
const apiOf = (rows: Schedule[]) => ({
  request: vi.fn(async (path: string) => {
    if (path === "/schedules") return json(rows, { limits: { min_change_interval_minutes: 15, max_files: { verify: 500, prepare: 12 }, max_files_video: 6 } });
    if (path === "/roots") return json([]);
    return json({});
  }),
});

const names = (card: HTMLElement) => within(card).getAllByRole("button").map((button) => button.getAttribute("aria-label") ?? button.textContent?.trim());

describe("a schedule card", () => {
  it("keeps the frequent actions at the left of its band and the rare ones at the right", async () => {
    const view = render(() => <SchedulesPage api={apiOf([schedule()])} />);
    const card = await view.findByRole("article", { name: "Nightly verify" });
    const band = card.querySelector(".sched-actions") as HTMLElement;
    const left = band.querySelector(".sched-actions-main") as HTMLElement;
    const right = band.querySelector(".sched-actions-end") as HTMLElement;
    expect(names(left)).toEqual(["Pause Nightly verify", "Run now", "Edit"]);
    expect(names(right)).toEqual(["Duplicate", "Delete", "History of Nightly verify"]);
  });

  it("offers Turn on, not Pause, for a paused schedule", async () => {
    const view = render(() => <SchedulesPage api={apiOf([schedule({ enabled: false, next_run_at: null })])} />);
    const card = await view.findByRole("article", { name: "Nightly verify" });
    expect(within(card).getByRole("button", { name: "Turn on Nightly verify" })).toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: "Pause Nightly verify" })).toBeNull();
  });

  it("labels each column and shows the state as a chip", async () => {
    const view = render(() => <SchedulesPage api={apiOf([schedule()])} />);
    const card = await view.findByRole("article", { name: "Nightly verify" });
    const columns = Array.from(card.querySelectorAll(".sched > div"));
    expect(columns).toHaveLength(5);
    expect(columns[1]).toHaveTextContent("Timing");
    expect(columns[2]).toHaveTextContent("Scope");
    expect(columns[3]).toHaveTextContent("Next run");
    expect(columns[3]).toHaveTextContent("Last run");
    expect(columns[4]).toHaveTextContent("On");
  });
});

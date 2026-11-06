import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { SchedulesPage } from "../src/features/schedules/SchedulesPage";
import { describeDays, describeResult, describeTarget, stateChoices } from "../src/features/schedules/model";
import type { Schedule } from "../src/features/schedules/loaders";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

const schedule = (over: Partial<Schedule> = {}): Schedule => ({
  id: "sc_1", name: "Nightly verify", action: "verify", root_id: "r_1", root_name: "Film", compliance_state: "unknown",
  cron: "0 3 * * 1,2,3,4,5", days: [0, 1, 2, 3, 4], time: "03:00", max_files: 25, enabled: true, matching: 12,
  next_run_at: "2026-10-26T02:00:00+00:00", last_run_at: null, last_result: null, last_job_id: null, ...over,
});

const scanSchedule = (over: Partial<Schedule> = {}): Schedule =>
  schedule({ id: "sc_s", name: "Nightly scan", action: "scan", compliance_state: null, max_files: null, matching: null, ...over });

const json = (data: unknown, status = 200) => new Response(JSON.stringify({ data, meta: {} }), { status });

function makeApi(initial: Schedule[], handlers: Record<string, (init?: RequestInit) => Response> = {}, options: { deleteOriginal?: boolean } = {}) {
  let current = initial;
  const calls: Array<{ path: string; init?: RequestInit }> = [];
  const api: Api = {
    request: vi.fn(async (path: string, init?: RequestInit) => {
      calls.push({ path, init });
      const method = init?.method ?? "GET";
      const handler = handlers[`${method} ${path}`];
      if (handler) return handler(init);
      if (path === "/schedules" && method === "GET") return json(current);
      if (path === "/dashboard") return json({ files: {}, jobs: {}, delete_original: options.deleteOriginal ?? false });
      if (path === "/roots") return json([{ id: "r_1", name: "Film", latest_scan: null }, { id: "r_2", name: "TV", latest_scan: null }]);
      return json({});
    }),
  };
  return { api, calls, set: (next: Schedule[]) => { current = next; } };
}

describe("schedule wording", () => {
  it("names days the way a person says them", () => {
    expect(describeDays([0, 1, 2, 3, 4])).toBe("Weekdays");
    expect(describeDays([0, 1, 2, 3, 4, 5, 6])).toBe("Every day");
    expect(describeDays([5, 6])).toBe("Weekends");
    expect(describeDays([4, 0, 2])).toBe("Mon Wed Fri");
  });

  it("summarises what a schedule acts on", () => {
    expect(describeTarget({ action: "verify", root_name: "Film", compliance_state: "unknown", max_files: 25 })).toBe("Verify · Film · Not verified · up to 25 files");
    expect(describeTarget({ action: "fix", root_name: null, compliance_state: null, max_files: 1 })).toBe("Fix · All libraries · up to 1 file");
    expect(describeTarget({ action: "convert", root_name: "TV", compliance_state: null, max_files: 3 })).toBe("Convert · TV · up to 3 files");
    expect(describeTarget({ action: "scan", root_name: null, compliance_state: null, max_files: null })).toBe("Scan · All libraries");
    expect(describeTarget({ action: "scan", root_name: "Film", compliance_state: null, max_files: null })).toBe("Scan · Film");
  });

  it("puts the scan results of the server into words", () => {
    expect(describeResult("Queued 1 scan")).toBe("Queued 1 scan");
    expect(describeResult("Queued 2 scans")).toBe("Queued 2 scans");
    expect(describeResult("No libraries")).toBe("No libraries");
  });

  it("offers state filters that fit the action", () => {
    expect(stateChoices("fix").map((choice) => choice.label)).toEqual(["Any file that needs work", "Warning", "Blocker"]);
    expect(stateChoices("convert").map((choice) => choice.value)).toEqual([null, "warning", "blocker"]);
    expect(stateChoices("scan")).toEqual([]);
    expect(stateChoices("verify")[0]?.value).toBe("unknown");
  });
});

describe("SchedulesPage", () => {
  it("teaches the page when there are no schedules", async () => {
    const { api } = makeApi([]);
    const view = render(() => <SchedulesPage api={api} />);

    expect(await view.findByText("No schedules yet")).toBeInTheDocument();
    expect(view.getByText(/only reads files/)).toBeInTheDocument();
  });

  it("offers only Verify and Scan: a schedule never changes files", async () => {
    const { api } = makeApi([]);
    const view = render(() => <SchedulesPage api={api} />);
    await fireEvent.click(await view.findByRole("button", { name: /New schedule/ }));
    const editor = await view.findByRole("region", { name: "New schedule" });
    const picker = within(editor).getByRole("group", { name: "Action" });

    expect(within(picker).getAllByRole("button").map((button) => button.textContent)).toEqual(["Verify", "Scan"]);
    expect(within(editor).queryByRole("note")).toBeNull();
  });

  it("lists each schedule with its target, timetable, next run and last result", async () => {
    const { api } = makeApi([schedule({ last_run_at: "2026-10-23T01:00:00+00:00", last_result: "Queued 12 files", last_job_id: "j_9" })]);
    const view = render(() => <SchedulesPage api={api} />);

    const list = await view.findByRole("list", { name: "Schedules" });
    expect(list).toHaveTextContent("Nightly verify");
    expect(list).toHaveTextContent("Verify · Film · Not verified · up to 25 files");
    expect(list).toHaveTextContent("Weekdays · 03:00");
    expect(list).toHaveTextContent("Queued 12 files");
    expect(list).toHaveTextContent("12 files match now");
    expect(within(list).getByRole("link", { name: "Open job" })).toHaveAttribute("href", "/jobs/j_9");
  });

  it("shows a paused schedule as paused", async () => {
    const { api } = makeApi([schedule({ enabled: false, next_run_at: null })]);
    const view = render(() => <SchedulesPage api={api} />);

    expect(await view.findByText("Paused")).toBeInTheDocument();
    expect(view.getByRole("checkbox", { name: "Turn on Nightly verify" })).not.toBeChecked();
  });

  it("creates a verify schedule from the editor, and it is on", async () => {
    let body: unknown;
    const state = makeApi([], { "POST /schedules": (init) => { body = JSON.parse(String(init?.body)); return json(schedule({ action: "verify", enabled: true }), 201); } });
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("button", { name: /New schedule/ }));
    const editor = await view.findByRole("region", { name: "New schedule" });
    expect(within(editor).getByRole("button", { name: "Save schedule" })).toBeDisabled();
    await fireEvent.input(within(editor).getByLabelText("Name"), { target: { value: "Weekly check" } });
    await fireEvent.click(within(editor).getByRole("button", { name: "Saturday" }));
    await fireEvent.click(within(editor).getByRole("button", { name: "Save schedule" }));

    await waitFor(() => expect(body).toEqual({
      name: "Weekly check", action: "verify", root_id: null, compliance_state: "unknown",
      days: [0, 1, 2, 3, 4, 5], time: "03:00", max_files: 25,
    }));
    expect(await view.findByRole("status")).toHaveTextContent("Schedule saved and on.");
  });

  it("turns a verify schedule on and off at once", async () => {
    const patches: unknown[] = [];
    const state = makeApi(
      [schedule({ enabled: false, next_run_at: null })],
      { "PATCH /schedules/sc_1": (init) => { patches.push(JSON.parse(String(init?.body))); return json(schedule()); } },
    );
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("checkbox", { name: "Turn on Nightly verify" }));

    await waitFor(() => expect(patches).toEqual([{ enabled: true }]));
  });

  it("cannot turn on or run an old schedule that would change files", async () => {
    const calls: string[] = [];
    const state = makeApi(
      [schedule({ id: "sc_f", name: "Weekly fix", action: "fix", enabled: false, next_run_at: null, compliance_state: null })],
      { "PATCH /schedules/sc_f": () => { calls.push("patch"); return json(schedule()); }, "POST /schedules/sc_f/run": () => { calls.push("run"); return json(schedule()); } },
    );
    const view = render(() => <SchedulesPage api={state.api} />);

    expect(await view.findByRole("checkbox", { name: "Turn on Weekly fix" })).toBeDisabled();
    expect(view.getByRole("button", { name: "Run now" })).toBeDisabled();
    expect(calls).toEqual([]);
  });

  it("still shows an old repair or conversion schedule under its own name", async () => {
    const { api } = makeApi([
      schedule({ id: "sc_f", name: "Weekly fix", action: "fix", enabled: false, next_run_at: null, compliance_state: null }),
      schedule({ id: "sc_c", name: "Weekly convert", action: "convert", enabled: false, next_run_at: null, compliance_state: null }),
    ]);
    const view = render(() => <SchedulesPage api={api} />);

    const list = await view.findByRole("list", { name: "Schedules" });
    expect(list).toHaveTextContent("Fix · Film · up to 25 files");
    expect(list).toHaveTextContent("Convert · Film · up to 25 files");
    expect(list.textContent).not.toMatch(/prepare/i);
  });

  it("creates a scan schedule without file filter or files per run", async () => {
    let body: unknown;
    const state = makeApi([], { "POST /schedules": (init) => { body = JSON.parse(String(init?.body)); return json(scanSchedule(), 201); } });
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("button", { name: /New schedule/ }));
    const editor = await view.findByRole("region", { name: "New schedule" });
    expect(within(editor).getByLabelText("Files per run")).toBeInTheDocument();
    await fireEvent.click(within(within(editor).getByRole("group", { name: "Action" })).getByRole("button", { name: "Scan" }));
    expect(within(editor).queryByLabelText("Files per run")).toBeNull();
    expect(within(editor).queryByLabelText("Files that are")).toBeNull();
    await fireEvent.input(within(editor).getByLabelText("Name"), { target: { value: "Nightly scan" } });
    await fireEvent.click(within(editor).getByRole("button", { name: "Save schedule" }));

    await waitFor(() => expect(body).toEqual({
      name: "Nightly scan", action: "scan", root_id: null, days: [0, 1, 2, 3, 4], time: "03:00",
    }));
    expect(await view.findByRole("status")).toHaveTextContent("Schedule saved and on.");
  });

  it("lists a scan schedule without a file count, and turns it on and runs it now", async () => {
    const runs: string[] = [];
    const patches: unknown[] = [];
    const state = makeApi([scanSchedule({ enabled: false, next_run_at: null, last_run_at: "2026-10-23T01:00:00+00:00", last_result: "Queued 1 scan" })], {
      "POST /schedules/sc_s/run": () => { runs.push("scan"); return json(scanSchedule()); },
      "PATCH /schedules/sc_s": (init) => { patches.push(JSON.parse(String(init?.body))); return json(scanSchedule()); },
    });
    const view = render(() => <SchedulesPage api={state.api} />);

    const list = await view.findByRole("list", { name: "Schedules" });
    expect(list).toHaveTextContent("Scan · Film");
    expect(list).toHaveTextContent("Queued 1 scan");
    expect(list.textContent).not.toMatch(/files? match/);
    await fireEvent.click(view.getByRole("checkbox", { name: "Turn on Nightly scan" }));
    await waitFor(() => expect(patches).toEqual([{ enabled: true }]));
    const runNow = await view.findByRole("button", { name: "Run now" });
    await waitFor(() => expect(runNow).toBeEnabled());
    await fireEvent.click(runNow);
    await waitFor(() => expect(runs).toEqual(["scan"]));
  });

  it("runs a verify at once", async () => {
    const runs: string[] = [];
    const state = makeApi([schedule()], { "POST /schedules/sc_1/run": () => { runs.push("verify"); return json(schedule()); } });
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("button", { name: "Run now" }));

    await waitFor(() => expect(runs).toEqual(["verify"]));
    await view.findByText("Run started. The result is on the schedule.");
  });

  it("deletes only after confirmation", async () => {
    const deleted: string[] = [];
    const state = makeApi([schedule()], { "DELETE /schedules/sc_1": () => { deleted.push("sc_1"); return json({ deleted: true }); } });
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("button", { name: "Delete" }));
    expect(deleted).toEqual([]);
    await fireEvent.click(await view.findByRole("button", { name: "Confirm delete" }));

    await waitFor(() => expect(deleted).toEqual(["sc_1"]));
  });

  it("shows why the server refused a save", async () => {
    const state = makeApi([], { "POST /schedules": () => new Response(JSON.stringify({ detail: "unknown library" }), { status: 422 }) });
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("button", { name: /New schedule/ }));
    const editor = await view.findByRole("region", { name: "New schedule" });
    await fireEvent.input(within(editor).getByLabelText("Name"), { target: { value: "x" } });
    await fireEvent.click(within(editor).getByRole("button", { name: "Save schedule" }));

    expect(await within(editor).findByRole("alert")).toHaveTextContent("The server said: unknown library");
  });

  it("opens the editor with N and closes it with Escape", async () => {
    const { api } = makeApi([schedule()]);
    const view = render(() => <SchedulesPage api={api} />);
    await view.findByRole("list", { name: "Schedules" });
    const page = view.container.querySelector("section.schedules") as HTMLElement;

    await fireEvent.keyDown(page, { key: "n" });
    const editor = await view.findByRole("region", { name: "New schedule" });
    await fireEvent.keyDown(editor, { key: "Escape" });

    await waitFor(() => expect(view.queryByRole("region", { name: "New schedule" })).not.toBeInTheDocument());
  });
});

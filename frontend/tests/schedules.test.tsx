import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { setLanguage } from "../src/lib/i18n";
import { SchedulesPage } from "../src/features/schedules/SchedulesPage";
import { describeDays, describeProblem, describeResult, describeTarget, simpleForm, stateChoices, weeklyCron } from "../src/features/schedules/model";
import type { Schedule } from "../src/features/schedules/loaders";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

const schedule = (over: Partial<Schedule> = {}): Schedule => ({
  id: "sc_1", name: "Nightly verify", action: "verify", root_id: "r_1", root_name: "Film", compliance_state: "unknown",
  cron: "0 3 * * 1,2,3,4,5", days: [0, 1, 2, 3, 4], time: "03:00", max_files: 25, enabled: true, matching: 12,
  next_run_at: "2026-10-26T02:00:00+00:00", last_run_at: null, last_result: null, last_job_id: null, ...over,
});

const scanSchedule = (over: Partial<Schedule> = {}): Schedule =>
  schedule({ id: "sc_s", name: "Nightly scan", action: "scan", compliance_state: null, max_files: null, matching: null, ...over });

const LIMITS = { min_change_interval_minutes: 15, max_files: { verify: 500, fix: 12, convert: 6 } };
const json = (data: unknown, status = 200, meta: unknown = {}) => new Response(JSON.stringify({ data, meta }), { status });

function makeApi(initial: Schedule[], handlers: Record<string, (init?: RequestInit) => Response> = {}, options: { deleteOriginal?: boolean; limits?: unknown } = {}) {
  let current = initial;
  const calls: Array<{ path: string; init?: RequestInit }> = [];
  const api: Api = {
    request: vi.fn(async (path: string, init?: RequestInit) => {
      calls.push({ path, init });
      const method = init?.method ?? "GET";
      const handler = handlers[`${method} ${path}`];
      if (handler) return handler(init);
      if (path === "/schedules" && method === "GET") return json(current, 200, { limits: options.limits ?? LIMITS });
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

  it("explains a run that was refused for lack of approval", () => {
    expect(describeResult("Not queued: not approved to change files. Turn the schedule on again.")).toBe("Not queued: Not approved to change files. Turn the schedule on again.");
  });

  it("explains why a schedule was paused for breaking a limit", () => {
    expect(describeResult("Paused: Files per run may be at most 12 for this action.")).toBe("Paused: it takes more than 12 files per run. Adjust the schedule.");
    expect(describeResult("Paused: A schedule that changes files may run at most every 15 minutes; this one runs every 10.")).toBe("Paused: it runs more often than every 15 minutes (every 10). Adjust the schedule.");
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
    expect(view.getByText(/starts paused/)).toBeInTheDocument();
  });

  it("offers Verify, Fix, Convert and Scan", async () => {
    const { api } = makeApi([]);
    const view = render(() => <SchedulesPage api={api} />);
    await fireEvent.click(await view.findByRole("button", { name: /New schedule/ }));
    const editor = await view.findByRole("region", { name: "New schedule" });
    const picker = within(editor).getByRole("group", { name: "Action" });

    expect(within(picker).getAllByRole("button").map((button) => button.textContent)).toEqual(["Verify", "Fix", "Convert", "Scan"]);
    expect(within(editor).queryByRole("note")).toBeNull();
  });

  it("unfolds the history of a schedule with its results and job links", async () => {
    const runs: Schedule["runs"] = [
      { started_at: "2026-10-23T01:00:00+00:00", trigger: "scheduled", result: "Queued 12 files", job_id: "j_9", scan_id: null },
      { started_at: "2026-10-22T09:00:00+00:00", trigger: "manual", result: "No matching files", job_id: null, scan_id: null },
    ];
    const { api } = makeApi([schedule({ runs })]);
    const view = render(() => <SchedulesPage api={api} />);

    const toggle = await view.findByRole("button", { name: "History of Nightly verify" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(view.queryByRole("list", { name: "History of Nightly verify" })).toBeNull();
    fireEvent.click(toggle);
    const history = await view.findByRole("list", { name: "History of Nightly verify" });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const entries = within(history).getAllByRole("listitem");
    expect(entries).toHaveLength(2);
    expect(entries[0]).toHaveTextContent("Queued 12 files");
    expect(within(entries[0]!).getByRole("link", { name: "Open job" })).toHaveAttribute("href", "/jobs/j_9");
    expect(entries[1]).toHaveTextContent("No matching files");
    expect(within(entries[1]!).queryByRole("link")).toBeNull();
    fireEvent.click(toggle);
    expect(view.queryByRole("list", { name: "History of Nightly verify" })).toBeNull();
  });

  it("shows a missed slot in the history with a hint to adjust the schedule", async () => {
    const runs: Schedule["runs"] = [
      { kind: "missed", started_at: "2026-10-23T20:00:00+00:00", trigger: "scheduled", result: "Missed: not started within 360 minutes of its slot", job_id: null, scan_id: null },
      { kind: "run", started_at: "2026-10-22T01:00:00+00:00", trigger: "scheduled", result: "Queued 3 files", job_id: "j_2", scan_id: null },
    ];
    const { api } = makeApi([schedule({ runs })]);
    const view = render(() => <SchedulesPage api={api} />);

    fireEvent.click(await view.findByRole("button", { name: "History of Nightly verify" }));
    const entries = within(await view.findByRole("list", { name: "History of Nightly verify" })).getAllByRole("listitem");
    expect(entries[0]).toHaveTextContent("Missed: not started within 360 minutes of its slot");
    expect(entries[0]).toHaveTextContent("Adjust the schedule or check that the manager is running.");
    expect(entries[1]).not.toHaveTextContent("Adjust the schedule");
  });

  it("offers no history for a schedule that never ran", async () => {
    const { api } = makeApi([schedule({ runs: [] })]);
    const view = render(() => <SchedulesPage api={api} />);
    await view.findByRole("list", { name: "Schedules" });
    expect(view.queryByRole("button", { name: /History of/ })).toBeNull();
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

    const list = await view.findByRole("list", { name: "Schedules" });
    expect(within(list).getByText("Paused")).toBeInTheDocument();
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

  it("asks before turning on a schedule that changes files, and sends the confirmation", async () => {
    const patches: unknown[] = [];
    const state = makeApi(
      [schedule({ id: "sc_f", name: "Weekly fix", action: "fix", enabled: false, next_run_at: null, compliance_state: null, max_files: 10 })],
      { "PATCH /schedules/sc_f": (init) => { patches.push(JSON.parse(String(init?.body))); return json(schedule()); } },
    );
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("checkbox", { name: "Turn on Weekly fix" }));
    const question = await view.findByRole("region", { name: "Turn on Weekly fix" });
    expect(question).toHaveTextContent("up to 10");
    expect(question).toHaveTextContent("Originals are kept");
    expect(patches).toEqual([]);
    await fireEvent.click(within(question).getByRole("button", { name: "Cancel" }));
    expect(patches).toEqual([]);
    expect(view.queryByRole("region", { name: "Turn on Weekly fix" })).toBeNull();

    await fireEvent.click(view.getByRole("checkbox", { name: "Turn on Weekly fix" }));
    await fireEvent.click(within(await view.findByRole("region", { name: "Turn on Weekly fix" })).getByRole("button", { name: "Confirm turn on" }));
    await waitFor(() => expect(patches).toEqual([{ enabled: true, confirm_changes_files: true }]));
  });

  it("tells the current match count, capped by the run size, and that originals are deleted", async () => {
    const state = makeApi(
      [schedule({ id: "sc_f", name: "Weekly fix", action: "fix", enabled: false, compliance_state: null, max_files: 10, matching: 4 })],
      {}, { deleteOriginal: true },
    );
    const view = render(() => <SchedulesPage api={state.api} />);
    await view.findByText("Weekly fix");
    state.set([schedule({ id: "sc_f", name: "Weekly fix", action: "fix", enabled: false, compliance_state: null, max_files: 10, matching: 37 })]);

    await fireEvent.click(await view.findByRole("checkbox", { name: "Turn on Weekly fix" }));
    const question = await view.findByRole("region", { name: "Turn on Weekly fix" });

    expect(question).toHaveTextContent("10 files match now");
    expect(question).toHaveTextContent("Originals are deleted");
    expect(question).toHaveTextContent("approval applies to these settings only");
  });

  it("turns on a schedule that only checks or scans without any question", async () => {
    const patches: unknown[] = [];
    const state = makeApi(
      [scanSchedule({ enabled: false, id: "sc_s" })],
      { "PATCH /schedules/sc_s": (init) => { patches.push(JSON.parse(String(init?.body))); return json(scanSchedule()); } },
      { deleteOriginal: true },
    );
    const view = render(() => <SchedulesPage api={state.api} />);
    await fireEvent.click(await view.findByRole("checkbox", { name: "Turn on Nightly scan" }));
    await waitFor(() => expect(patches).toEqual([{ enabled: true }]));
    expect(view.queryByRole("region", { name: "Turn on Nightly scan" })).toBeNull();
  });

  it("pauses a schedule that changes files without asking", async () => {
    const patches: unknown[] = [];
    const state = makeApi(
      [schedule({ id: "sc_c", name: "Weekly convert", action: "convert", enabled: true, compliance_state: null })],
      { "PATCH /schedules/sc_c": (init) => { patches.push(JSON.parse(String(init?.body))); return json(schedule()); } },
    );
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("checkbox", { name: "Pause Weekly convert" }));

    await waitFor(() => expect(patches).toEqual([{ enabled: false }]));
  });

  it("can run a schedule that changes files now", async () => {
    const runs: string[] = [];
    const state = makeApi(
      [schedule({ id: "sc_f", name: "Weekly fix", action: "fix", compliance_state: null })],
      { "POST /schedules/sc_f/run": () => { runs.push("fix"); return json(schedule()); } },
    );
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("button", { name: "Run now" }));

    await waitFor(() => expect(runs).toEqual(["fix"]));
  });

  it("creates a fix schedule that starts paused and needs its files per run", async () => {
    let body: unknown;
    const state = makeApi([], { "POST /schedules": (init) => { body = JSON.parse(String(init?.body)); return json(schedule({ action: "fix", enabled: false }), 201); } });
    const view = render(() => <SchedulesPage api={state.api} />);

    await fireEvent.click(await view.findByRole("button", { name: /New schedule/ }));
    const editor = await view.findByRole("region", { name: "New schedule" });
    await fireEvent.click(within(within(editor).getByRole("group", { name: "Action" })).getByRole("button", { name: "Fix" }));
    await fireEvent.input(within(editor).getByLabelText("Name"), { target: { value: "Nightly fix" } });
    await fireEvent.input(within(editor).getByLabelText("Files per run"), { target: { value: "0" } });
    expect(within(editor).getByRole("button", { name: "Save schedule" })).toBeDisabled();
    await fireEvent.input(within(editor).getByLabelText("Files per run"), { target: { value: "7" } });
    await fireEvent.click(within(editor).getByRole("button", { name: "Save schedule" }));

    await waitFor(() => expect(body).toEqual({
      name: "Nightly fix", action: "fix", root_id: null, compliance_state: null,
      days: [0, 1, 2, 3, 4], time: "03:00", max_files: 7,
    }));
    expect(await view.findByRole("status")).toHaveTextContent("Schedule saved. It is paused until you turn it on.");
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

  it("shows the server's refusal of a limit in the chosen language", async () => {
    setLanguage("de");
    try {
      expect(describeProblem("Files per run may be at most 12 for this action.")).toBe("Dateien pro Lauf dürfen bei dieser Aktion höchstens 12 betragen.");
      expect(describeProblem("A schedule that changes files may run at most every 15 minutes; this one runs every 10.")).toBe("Ein Zeitplan, der Dateien ändert, darf höchstens alle 15 Minuten laufen; dieser läuft alle 10.");
      expect(describeProblem("unknown library")).toBe("unknown library");
      const state = makeApi([], { "POST /schedules": () => new Response(JSON.stringify({ detail: "Files per run may be at most 12 for this action." }), { status: 422 }) });
      const view = render(() => <SchedulesPage api={state.api} />);
      await fireEvent.click(await view.findByRole("button", { name: /Neuer Zeitplan/ }));
      const editor = await view.findByRole("region", { name: "Neuer Zeitplan" });
      await fireEvent.input(within(editor).getByLabelText("Name"), { target: { value: "x" } });
      await fireEvent.click(within(editor).getByRole("button", { name: "Zeitplan speichern" }));
      expect(await within(editor).findByRole("alert")).toHaveTextContent("Dateien pro Lauf dürfen bei dieser Aktion höchstens 12 betragen.");
    } finally {
      setLanguage("en");
    }
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

const preview = (cron: string, description: string, runs: string[] = ['2026-10-26T02:00:00+00:00', '2026-10-27T02:00:00+00:00', '2026-10-28T02:00:00+00:00', '2026-10-29T02:00:00+00:00', '2026-10-30T02:00:00+00:00']) =>
  json({ valid: true, expression: cron, description, next_runs: runs });
const invalid = (error: string) => json({ valid: false, error });

describe('cron expressions in the simple mode', () => {
  it('turns days and a time into the cron the server would store', () => {
    expect(weeklyCron([0, 1, 2, 3, 4], '03:00')).toBe('0 3 * * 1,2,3,4,5');
    expect(weeklyCron([6, 0], '03:05')).toBe('5 3 * * 0,1');
  });

  it('treats weekday 7 as Sunday, like 0 and like the server', () => {
    expect(simpleForm('0 3 * * 0,7')).toEqual({ days: [6], time: '03:00' });
    expect(simpleForm('0 3 * * 5-7')).toEqual({ days: [4, 5, 6], time: '03:00' });
    expect(simpleForm('0 3 * * 0-7')).toEqual({ days: [0, 1, 2, 3, 4, 5, 6], time: '03:00' });
    expect(simpleForm('0 3 * * 1-7')).toEqual({ days: [0, 1, 2, 3, 4, 5, 6], time: '03:00' });
    expect(simpleForm('0 3 * * 7-0')).toBeNull();
    // What the editor writes back for Sunday is 0, which says the same as 7.
    expect(weeklyCron([6], '03:00')).toBe('0 3 * * 0');
  });

  it('recognises what days and a time can express, and nothing else', () => {
    expect(simpleForm('0 3 * * 1-5')).toEqual({ days: [0, 1, 2, 3, 4], time: '03:00' });
    expect(simpleForm('5 3 * * 0,1')).toEqual({ days: [0, 6], time: '03:05' });
    expect(simpleForm('30 22 * * 7')).toEqual({ days: [6], time: '22:30' });
    expect(simpleForm('0 3 * * *')).toEqual({ days: [0, 1, 2, 3, 4, 5, 6], time: '03:00' });
    for (const cron of ['*/15 * * * *', '0 */6 * * *', '0 3 1 * *', '0 3 * 6 *', '0,30 3 * * *', '0 3,4 * * *', '0 3 * * 1-5/2', '0 3 * * 9']) {
      expect(simpleForm(cron), cron).toBeNull();
    }
  });
});

describe('the cron editor', () => {
  const openCron = async (state: ReturnType<typeof makeApi>) => {
    const view = render(() => <SchedulesPage api={state.api} />);
    await fireEvent.click(await view.findByRole('button', { name: /New schedule/ }));
    const editor = await view.findByRole('region', { name: 'New schedule' });
    await fireEvent.click(within(editor).getByRole('button', { name: 'Cron' }));
    return { view, editor };
  };

  it('switches between Simple and Cron and starts Cron from the days and time', async () => {
    const state = makeApi([], { 'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en&action=verify': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays') });
    const { editor } = await openCron(state);

    expect(within(editor).getByLabelText(/Cron expression/)).toHaveValue('0 3 * * 1,2,3,4,5');
    expect(within(editor).queryByLabelText(/Time \(server time\)/)).toBeNull();
    expect(within(editor).getByRole('button', { name: 'Cron' })).toHaveAttribute('aria-pressed', 'true');
    await fireEvent.click(within(editor).getByRole('button', { name: 'Simple' }));
    expect(within(editor).getByLabelText(/Time \(server time\)/)).toBeInTheDocument();
    expect(within(editor).queryByLabelText(/Cron expression/)).toBeNull();
  });

  it('describes the expression and lists the next five runs while typing', async () => {
    const state = makeApi([], {
      'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en&action=verify': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays'),
      'GET /schedules/cron-preview?expression=*%2F15%20*%20*%20*%20*&lang=en&action=verify': () => preview('*/15 * * * *', 'Every 15 minutes', ['2026-10-26T02:15:00+00:00', '2026-10-26T02:30:00+00:00', '2026-10-26T02:45:00+00:00', '2026-10-26T03:00:00+00:00', '2026-10-26T03:15:00+00:00']),
    });
    const { editor } = await openCron(state);
    expect(await within(editor).findByText('At 03:00, on weekdays')).toBeInTheDocument();

    await fireEvent.input(within(editor).getByLabelText(/Cron expression/), { target: { value: '*/15 * * * *' } });

    expect(await within(editor).findByText('Every 15 minutes')).toBeInTheDocument();
    const runs = within(editor).getByRole('list', { name: 'Next runs' });
    expect(within(runs).getAllByRole('listitem')).toHaveLength(5);
  });

  it('shows what is wrong with an invalid expression and blocks saving', async () => {
    const state = makeApi([], {
      'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en&action=verify': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays'),
      'GET /schedules/cron-preview?expression=61%20*%20*%20*%20*&lang=en&action=verify': () => invalid('minute: 61 is out of range 0-59'),
    });
    const { editor } = await openCron(state);
    await fireEvent.input(within(editor).getByLabelText('Name'), { target: { value: 'x' } });
    await within(editor).findByText('At 03:00, on weekdays');
    expect(within(editor).getByRole('button', { name: 'Save schedule' })).toBeEnabled();

    await fireEvent.input(within(editor).getByLabelText(/Cron expression/), { target: { value: '61 * * * *' } });

    expect(await within(editor).findByText('minute: 61 is out of range 0-59')).toBeInTheDocument();
    expect(within(editor).getByLabelText(/Cron expression/)).toHaveAttribute('aria-invalid', 'true');
    expect(within(editor).getByRole('button', { name: 'Save schedule' })).toBeDisabled();
    expect(within(editor).queryByRole('list', { name: 'Next runs' })).toBeNull();
  });

  it('saves the expression itself', async () => {
    let body: unknown;
    const state = makeApi([], {
      'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en&action=verify': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays'),
      'GET /schedules/cron-preview?expression=0%20*%2F6%20*%20*%20*&lang=en&action=verify': () => preview('0 */6 * * *', 'At 00:00, 06:00, 12:00, 18:00, every day'),
      'POST /schedules': (init) => { body = JSON.parse(String(init?.body)); return json(schedule(), 201); },
    });
    const { editor } = await openCron(state);
    await fireEvent.input(within(editor).getByLabelText('Name'), { target: { value: 'Often' } });
    await fireEvent.input(within(editor).getByLabelText(/Cron expression/), { target: { value: '0 */6 * * *' } });
    await within(editor).findByText('At 00:00, 06:00, 12:00, 18:00, every day');
    await fireEvent.click(within(editor).getByRole('button', { name: 'Save schedule' }));

    await waitFor(() => expect(body).toEqual({ name: 'Often', action: 'verify', root_id: null, compliance_state: 'unknown', cron: '0 */6 * * *', max_files: 25 }));
  });

  it('opens a schedule that days and time cannot express directly in Cron mode', async () => {
    const state = makeApi([schedule({ cron: '*/15 * * * *', days: [0, 1, 2, 3, 4, 5, 6], time: '00:00' })], {
      'GET /schedules/cron-preview?expression=*%2F15%20*%20*%20*%20*&lang=en&action=verify': () => preview('*/15 * * * *', 'Every 15 minutes'),
    });
    const view = render(() => <SchedulesPage api={state.api} />);
    await fireEvent.click(await view.findByRole('button', { name: 'Edit' }));
    const editor = await view.findByRole('region', { name: 'Edit schedule' });

    expect(within(editor).getByRole('button', { name: 'Cron' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(editor).getByLabelText(/Cron expression/)).toHaveValue('*/15 * * * *');
    expect(await within(editor).findByText('Every 15 minutes')).toBeInTheDocument();
  });

  it('opens a weekly schedule in Simple mode', async () => {
    const state = makeApi([schedule()]);
    const view = render(() => <SchedulesPage api={state.api} />);
    await fireEvent.click(await view.findByRole('button', { name: 'Edit' }));
    const editor = await view.findByRole('region', { name: 'Edit schedule' });

    expect(within(editor).getByRole('button', { name: 'Simple' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(editor).getByLabelText(/Time \(server time\)/)).toHaveValue('03:00');
  });

  it('shows a cron schedule as its expression in the list', async () => {
    const state = makeApi([schedule({ cron: '0 */6 * * *', days: [0, 1, 2, 3, 4, 5, 6], time: '00:00' })]);
    const view = render(() => <SchedulesPage api={state.api} />);

    expect(await view.findByRole('list', { name: 'Schedules' })).toHaveTextContent('0 */6 * * *');
  });

  it('has a guide with the fields and examples that fill the field', async () => {
    const state = makeApi([], {
      'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en&action=verify': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays'),
      'GET /schedules/cron-preview?expression=*%2F15%20*%20*%20*%20*&lang=en&action=verify': () => preview('*/15 * * * *', 'Every 15 minutes'),
    });
    const { editor } = await openCron(state);
    expect(within(editor).queryByText(/day of month/)).toBeNull();

    const toggle = within(editor).getByRole('button', { name: 'Cron guide' });
    await fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(within(editor).getByText(/minute, hour, day of month, month, weekday/)).toBeInTheDocument();
    for (const example of ['0 3 * * 1-5', '*/15 * * * *', '0 */6 * * *']) {
      expect(within(editor).getByRole('button', { name: example })).toBeInTheDocument();
    }
    await fireEvent.click(within(editor).getByRole('button', { name: '*/15 * * * *' }));

    expect(within(editor).getByLabelText(/Cron expression/)).toHaveValue('*/15 * * * *');
    expect(await within(editor).findByText('Every 15 minutes')).toBeInTheDocument();
  });
});

describe("duplicating a schedule", () => {
  const open = async (state: ReturnType<typeof makeApi>) => {
    const view = render(() => <SchedulesPage api={state.api} />);
    await fireEvent.click(await view.findByRole("button", { name: "Duplicate" }));
    return { view, editor: await view.findByRole("region", { name: "New schedule" }) };
  };

  it("opens a new-schedule editor prefilled with a copy, and saves it as a new schedule", async () => {
    let body: unknown;
    const state = makeApi([schedule({ cron: "0 3 * * 1,2,3,4,5" })], { "POST /schedules": (init) => { body = JSON.parse(String(init?.body)); return json(schedule({ id: "sc_2" }), 201); } });
    const { view, editor } = await open(state);

    expect(within(editor).getByLabelText("Name")).toHaveValue("Nightly verify (copy)");
    expect(within(editor).getByLabelText("Library")).toHaveValue("r_1");
    await fireEvent.click(within(editor).getByRole("button", { name: "Save schedule" }));

    await waitFor(() => expect(body).toEqual({
      name: "Nightly verify (copy)", action: "verify", root_id: "r_1", compliance_state: "unknown",
      days: [0, 1, 2, 3, 4], time: "03:00", max_files: 25,
    }));
    expect(await view.findByRole("status")).toHaveTextContent("Schedule saved and on.");
    expect(state.calls.some((call) => call.init?.method === "PUT" || call.init?.method === "PATCH")).toBe(false);
  });

  it("keeps a cron expression and saves it", async () => {
    let body: unknown;
    const state = makeApi([schedule({ cron: "*/15 * * * *", days: [0, 1, 2, 3, 4, 5, 6], time: "00:00" })], {
      "GET /schedules/cron-preview?expression=*%2F15%20*%20*%20*%20*&lang=en&action=verify": () => preview("*/15 * * * *", "Every 15 minutes"),
      "POST /schedules": (init) => { body = JSON.parse(String(init?.body)); return json(schedule({ id: "sc_2" }), 201); },
    });
    const { editor } = await open(state);

    expect(within(editor).getByRole("button", { name: "Cron" })).toHaveAttribute("aria-pressed", "true");
    await within(editor).findByText("Every 15 minutes");
    await fireEvent.click(within(editor).getByRole("button", { name: "Save schedule" }));
    await waitFor(() => expect(body).toMatchObject({ name: "Nightly verify (copy)", cron: "*/15 * * * *", max_files: 25 }));
  });

  it("copies a fix schedule with its files per run, and the copy starts paused", async () => {
    let body: unknown;
    const state = makeApi([schedule({ action: "fix", compliance_state: "warning", max_files: 9 })], { "POST /schedules": (init) => { body = JSON.parse(String(init?.body)); return json(schedule({ id: "sc_2", action: "fix", enabled: false }), 201); } });
    const { view, editor } = await open(state);

    expect(within(editor).getByLabelText("Files per run")).toHaveValue(9);
    await fireEvent.click(within(editor).getByRole("button", { name: "Save schedule" }));
    await waitFor(() => expect(body).toMatchObject({ action: "fix", compliance_state: "warning", max_files: 9 }));
    expect(await view.findByRole("status")).toHaveTextContent("Schedule saved. It is paused until you turn it on.");
  });

  it("copies a scan schedule without files per run", async () => {
    let body: unknown;
    const state = makeApi([scanSchedule()], { "POST /schedules": (init) => { body = JSON.parse(String(init?.body)); return json(scanSchedule({ id: "sc_2" }), 201); } });
    const { editor } = await open(state);

    expect(within(editor).queryByLabelText("Files per run")).toBeNull();
    await fireEvent.click(within(editor).getByRole("button", { name: "Save schedule" }));
    await waitFor(() => expect(body).toMatchObject({ name: "Nightly scan (copy)", action: "scan" }));
    expect(body).not.toHaveProperty("max_files");
  });

  it("fits the copied name into the 80 character limit", async () => {
    const state = makeApi([schedule({ name: "x".repeat(80) })]);
    const { editor } = await open(state);
    const name = (within(editor).getByLabelText("Name") as HTMLInputElement).value;

    expect(name.length).toBe(80);
    expect(name.endsWith(" (copy)")).toBe(true);
  });
});

describe("limits read from the server", () => {
  const openEditor = async (state: ReturnType<typeof makeApi>) => {
    const view = render(() => <SchedulesPage api={state.api} />);
    await fireEvent.click(await view.findByRole("button", { name: /New schedule/ }));
    const editor = await view.findByRole("region", { name: "New schedule" });
    return { view, editor };
  };

  it("shows the limit of the chosen action at Files per run, with the server's numbers", async () => {
    const state = makeApi([], {}, { limits: { min_change_interval_minutes: 20, max_files: { verify: 321, fix: 9, convert: 4 } } });
    const { editor } = await openEditor(state);
    const files = () => within(editor).getByLabelText("Files per run");
    const choose = (name: string) => fireEvent.click(within(within(editor).getByRole("group", { name: "Action" })).getByRole("button", { name }));

    await waitFor(() => expect(files()).toHaveAttribute("max", "321"));
    expect(within(editor).getByText("At most 321 files per run.")).toBeInTheDocument();
    await choose("Fix");
    expect(files()).toHaveAttribute("max", "9");
    expect(within(editor).getByText("At most 9 files per run.")).toBeInTheDocument();
    await choose("Convert");
    expect(files()).toHaveAttribute("max", "4");
    await choose("Scan");
    expect(within(editor).queryByLabelText("Files per run")).toBeNull();
  });

  it("blocks a save above the limit and names it", async () => {
    const state = makeApi([]);
    const { editor } = await openEditor(state);
    await waitFor(() => expect(within(editor).getByLabelText("Files per run")).toHaveAttribute("max", "500"));
    await fireEvent.click(within(within(editor).getByRole("group", { name: "Action" })).getByRole("button", { name: "Convert" }));
    await fireEvent.input(within(editor).getByLabelText("Name"), { target: { value: "x" } });
    await fireEvent.input(within(editor).getByLabelText("Files per run"), { target: { value: "7" } });

    expect(within(editor).getByRole("button", { name: "Save schedule" })).toBeDisabled();
    expect(within(editor).getByText("Files per run must be 1 to 6.")).toBeInTheDocument();
    await fireEvent.input(within(editor).getByLabelText("Files per run"), { target: { value: "6" } });
    expect(within(editor).getByRole("button", { name: "Save schedule" })).toBeEnabled();
  });

  it("asks the server about the cron for the chosen action and shows its refusal", async () => {
    const state = makeApi([], {
      "GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en&action=fix": () => preview("0 3 * * 1,2,3,4,5", "At 03:00, on weekdays"),
      "GET /schedules/cron-preview?expression=0%2C10%20*%20*%20*%20*&lang=en&action=fix": () => invalid("A schedule that changes files may run at most every 15 minutes; this one runs every 10."),
    });
    const { editor } = await openEditor(state);
    await fireEvent.click(within(within(editor).getByRole("group", { name: "Action" })).getByRole("button", { name: "Fix" }));
    await fireEvent.input(within(editor).getByLabelText("Name"), { target: { value: "x" } });
    await fireEvent.click(within(editor).getByRole("button", { name: "Cron" }));
    await within(editor).findByText("At 03:00, on weekdays");

    await fireEvent.input(within(editor).getByLabelText(/Cron expression/), { target: { value: "0,10 * * * *" } });

    expect(await within(editor).findByText("A schedule that changes files may run at most every 15 minutes; this one runs every 10.")).toBeInTheDocument();
    expect(within(editor).getByText("A schedule that changes files may run at most every 15 minutes.")).toBeInTheDocument();
    expect(within(editor).getByRole("button", { name: "Save schedule" })).toBeDisabled();
  });
});

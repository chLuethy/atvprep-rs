import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import { SchedulesPage } from "../src/features/schedules/SchedulesPage";
import { describeDays, describeTarget, simpleForm, stateChoices, weeklyCron } from "../src/features/schedules/model";
import type { Schedule } from "../src/features/schedules/loaders";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

const schedule = (over: Partial<Schedule> = {}): Schedule => ({
  id: "sc_1", name: "Nightly verify", action: "verify", root_id: "r_1", root_name: "Film", compliance_state: "unknown",
  cron: "0 3 * * 1,2,3,4,5", days: [0, 1, 2, 3, 4], time: "03:00", max_files: 25, enabled: true, matching: 12,
  next_run_at: "2026-10-26T02:00:00+00:00", last_run_at: null, last_result: null, last_job_id: null, ...over,
});

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
    expect(describeTarget({ action: "prepare", root_name: null, compliance_state: null, max_files: 1 })).toBe("Prepare · All libraries · up to 1 file");
  });

  it("offers state filters that fit the action", () => {
    expect(stateChoices("prepare").map((choice) => choice.label)).toEqual(["Any file that needs work", "Warning", "Blocker"]);
    expect(stateChoices("prepare").map((choice) => choice.value)).toEqual([null, "warning", "blocker"]);
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

  it("offers only Verify: a schedule never changes files", async () => {
    const { api } = makeApi([]);
    const view = render(() => <SchedulesPage api={api} />);
    await fireEvent.click(await view.findByRole("button", { name: /New schedule/ }));
    const editor = await view.findByRole("region", { name: "New schedule" });
    const picker = within(editor).getByRole("group", { name: "Action" });

    expect(within(picker).getAllByRole("button").map((button) => button.textContent)).toEqual(["Verify"]);
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
      [schedule({ id: "sc_f", name: "Weekly fix", action: "prepare", enabled: false, next_run_at: null, compliance_state: null })],
      { "PATCH /schedules/sc_f": () => { calls.push("patch"); return json(schedule()); }, "POST /schedules/sc_f/run": () => { calls.push("run"); return json(schedule()); } },
    );
    const view = render(() => <SchedulesPage api={state.api} />);

    expect(await view.findByRole("checkbox", { name: "Turn on Weekly fix" })).toBeDisabled();
    expect(view.getByRole("button", { name: "Run now" })).toBeDisabled();
    expect(calls).toEqual([]);
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

const preview = (cron: string, description: string, runs: string[] = ['2026-10-26T02:00:00+00:00', '2026-10-27T02:00:00+00:00', '2026-10-28T02:00:00+00:00', '2026-10-29T02:00:00+00:00', '2026-10-30T02:00:00+00:00']) =>
  json({ valid: true, expression: cron, description, next_runs: runs });
const invalid = (error: string) => json({ valid: false, error });

describe('cron expressions in the simple mode', () => {
  it('turns days and a time into the cron the server would store', () => {
    expect(weeklyCron([0, 1, 2, 3, 4], '03:00')).toBe('0 3 * * 1,2,3,4,5');
    expect(weeklyCron([6, 0], '03:05')).toBe('5 3 * * 0,1');
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
    const state = makeApi([], { 'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays') });
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
      'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays'),
      'GET /schedules/cron-preview?expression=*%2F15%20*%20*%20*%20*&lang=en': () => preview('*/15 * * * *', 'Every 15 minutes', ['2026-10-26T02:15:00+00:00', '2026-10-26T02:30:00+00:00', '2026-10-26T02:45:00+00:00', '2026-10-26T03:00:00+00:00', '2026-10-26T03:15:00+00:00']),
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
      'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays'),
      'GET /schedules/cron-preview?expression=61%20*%20*%20*%20*&lang=en': () => invalid('minute: 61 is out of range 0-59'),
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
      'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays'),
      'GET /schedules/cron-preview?expression=0%20*%2F6%20*%20*%20*&lang=en': () => preview('0 */6 * * *', 'At 00:00, 06:00, 12:00, 18:00, every day'),
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
      'GET /schedules/cron-preview?expression=*%2F15%20*%20*%20*%20*&lang=en': () => preview('*/15 * * * *', 'Every 15 minutes'),
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
      'GET /schedules/cron-preview?expression=0%203%20*%20*%201%2C2%2C3%2C4%2C5&lang=en': () => preview('0 3 * * 1,2,3,4,5', 'At 03:00, on weekdays'),
      'GET /schedules/cron-preview?expression=*%2F15%20*%20*%20*%20*&lang=en': () => preview('*/15 * * * *', 'Every 15 minutes'),
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

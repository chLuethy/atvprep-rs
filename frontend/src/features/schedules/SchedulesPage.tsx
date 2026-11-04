import { createSignal, For, onMount, Show, type Component } from "solid-js";
import { Button, Kbd } from "../../ui";
import { N, t, tn } from "../../lib/i18n";
import { isTyping } from "../../lib/keys";
import { loadRoots, type RootRow } from "../scan/loaders";
import {
  createSchedule, deleteSchedule, loadSchedules, runScheduleNow, setEnabled, updateSchedule,
  type Action, type Api, type Schedule, type ScheduleInput,
} from "./loaders";
import {
  actionName, DAYS, defaultState, describeDays, describeLast, describeNext, describeTarget,
  EVERY_DAY, stateChoices, WEEKDAYS,
} from "./model";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");
type Phase = "loading" | "ready" | "error" | "session";
/** Which inline question a row is asking: turn on, run now, or delete. */
type Asking = { id: string; kind: "delete" } | null;

const blank = (): ScheduleInput => ({ name: "", action: "verify", rootId: null, complianceState: defaultState("verify"), days: [...WEEKDAYS], time: "03:00", maxFiles: 25 });

const problem = (detail: string | undefined, fallback: string) => (detail ? t("{fallback} The server said: {detail}.", { fallback, detail }) : fallback);

/** Weekly timetables that verify or prepare the files that match a filter. */
export const SchedulesPage: Component<{ api: Api }> = (props) => {
  const [rows, setRows] = createSignal<Schedule[]>([]);
  const [roots, setRoots] = createSignal<RootRow[]>([]);
  const [phase, setPhase] = createSignal<Phase>("loading");
  const [editing, setEditing] = createSignal<{ id: string | null } | null>(null);
  const [draft, setDraft] = createSignal<ScheduleInput>(blank());
  const [asking, setAsking] = createSignal<Asking>(null);
  const [busy, setBusy] = createSignal(false);
  const [formError, setFormError] = createSignal<string | null>(null);
  const [rowError, setRowError] = createSignal<string | null>(null);
  const [notice, setNotice] = createSignal<string | null>(null);
  let key = crypto.randomUUID();
  let newButton: HTMLButtonElement | undefined;
  let nameInput: HTMLInputElement | undefined;

  const fresh = () => { key = crypto.randomUUID(); };

  const load = async () => {
    const result = await loadSchedules(props.api);
    if (result.kind === "ok") { setRows(result.items); setPhase("ready"); }
    else setPhase(result.kind === "session" ? "session" : "error");
  };
  onMount(() => {
    void load();
    void loadRoots(props.api).then((result) => { if (result.kind === "ok") setRoots(result.items); });
  });

  const openNew = () => {
    fresh(); setFormError(null); setNotice(null); setAsking(null);
    setDraft(blank()); setEditing({ id: null });
    queueMicrotask(() => nameInput?.focus());
  };
  const openEdit = (schedule: Schedule) => {
    fresh(); setFormError(null); setNotice(null); setAsking(null);
    setDraft({ name: schedule.name, action: schedule.action, rootId: schedule.root_id, complianceState: schedule.compliance_state, days: [...schedule.days], time: schedule.time, maxFiles: schedule.max_files });
    setEditing({ id: schedule.id });
    queueMicrotask(() => nameInput?.focus());
  };
  const closeEditor = () => { setEditing(null); queueMicrotask(() => newButton?.focus()); };
  const patch = (changes: Partial<ScheduleInput>) => { fresh(); setDraft({ ...draft(), ...changes }); };

  const chooseAction = (action: Action) => {
    const states = stateChoices(action).map((choice) => choice.value);
    patch({ action, complianceState: states.includes(draft().complianceState) ? draft().complianceState : defaultState(action) });
  };
  const toggleDay = (day: number) => {
    const days = draft().days;
    patch({ days: days.includes(day) ? days.filter((known) => known !== day) : [...days, day].sort((a, b) => a - b) });
  };

  const problems = () => {
    const value = draft();
    const found: string[] = [];
    if (!value.name.trim()) found.push(t("Give the schedule a name."));
    if (value.days.length === 0) found.push(t("Pick at least one day."));
    if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(value.time)) found.push(t("Set a time."));
    if (!Number.isInteger(value.maxFiles) || value.maxFiles < 1 || value.maxFiles > 500) found.push(t("Files per run must be 1 to 500."));
    return found;
  };

  const save = async () => {
    const target = editing();
    if (!target || busy() || problems().length > 0) return;
    setBusy(true);
    setFormError(null);
    const input = { ...draft(), name: draft().name.trim() };
    const result = target.id === null
      ? await createSchedule(props.api, input, key)
      : await updateSchedule(props.api, target.id, input, key);
    setBusy(false);
    if (!result.ok) { setFormError(result.session ? t(SESSION_EXPIRED) : problem(result.detail, t("Unable to save the schedule."))); return; }
    setEditing(null);
    setNotice(target.id === null
      ? (input.action === "verify" ? t("Schedule saved and on.") : t("Schedule saved. It is paused until you turn it on."))
      : t("Schedule saved."));
    await load();
    queueMicrotask(() => newButton?.focus());
  };

  const ask = (id: string, kind: "delete") => { fresh(); setRowError(null); setNotice(null); setAsking({ id, kind }); };

  /** Run one row action, then reload. A failure stays on the row that caused it. */
  const act = async (run: () => Promise<{ ok: boolean; session?: boolean; detail?: string }>, done: string | null, failed: string) => {
    if (busy()) return;
    setBusy(true);
    setRowError(null);
    const result = await run();
    setBusy(false);
    if (!result.ok) { setRowError(result.session ? t(SESSION_EXPIRED) : problem(result.detail, failed)); return; }
    setAsking(null);
    if (done) setNotice(done);
    await load();
  };

  const toggle = (schedule: Schedule) => {
    fresh(); // a different request body must never reuse an earlier idempotency key
    void act(() => setEnabled(props.api, schedule.id, !schedule.enabled, key), null, t("Unable to change the schedule."));
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.metaKey || event.ctrlKey || event.altKey || isTyping(event.target)) return;
    if (event.key === "n" || event.key === "N") { event.preventDefault(); openNew(); }
    if (event.key === "Escape" && editing()) closeEditor();
  };

  const renderEditor = () => (
    <div
      class="confirm sched-editor"
      role="region"
      aria-label={editing()?.id === null ? t("New schedule") : t("Edit schedule")}
      onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); closeEditor(); } }}
    >
      <div class="sched-grid">
        <label class="sched-field wide">
          <span>{t("Name")}</span>
          <input ref={nameInput} class="input" type="text" maxLength={80} value={draft().name} onInput={(event) => patch({ name: event.currentTarget.value })} />
        </label>
        <div class="sched-field action">
          <span id="sched-action-label">{t("Action")}</span>
          <div class="seg" role="group" aria-labelledby="sched-action-label">
            <For each={["verify"] as Action[]}>{(action) => (
              <button type="button" aria-pressed={draft().action === action} disabled={editing()?.id !== null} onClick={() => chooseAction(action)}>{actionName(action)}</button>
            )}</For>
          </div>
        </div>
        <label class="sched-field">
          <span>{t("Library")}</span>
          <select class="select" value={draft().rootId ?? ""} onChange={(event) => patch({ rootId: event.currentTarget.value || null })}>
            <option value="">{t("All libraries")}</option>
            <For each={roots()}>{(root) => <option value={root.id}>{root.name}</option>}</For>
          </select>
        </label>
        <Show when={stateChoices(draft().action).length > 0}>
          <label class="sched-field">
            <span>{t("Files that are")}</span>
            <select class="select" value={draft().complianceState ?? ""} onChange={(event) => patch({ complianceState: event.currentTarget.value || null })}>
              <For each={stateChoices(draft().action)}>{(choice) => <option value={choice.value ?? ""}>{choice.label}</option>}</For>
            </select>
          </label>
        </Show>
        <div class="sched-field wide">
          <span id="sched-days-label">{t("Days")}</span>
          <div class="chips" role="group" aria-labelledby="sched-days-label">
            <For each={DAYS}>{(day) => (
              <button type="button" class="filter-chip" aria-pressed={draft().days.includes(day.index)} aria-label={t(day.long)} onClick={() => toggleDay(day.index)}>{t(day.short)}</button>
            )}</For>
            <Button variant="quiet" onClick={() => patch({ days: [...WEEKDAYS] })}>{t("Weekdays")}</Button>
            <Button variant="quiet" onClick={() => patch({ days: [...EVERY_DAY] })}>{t("Every day")}</Button>
          </div>
        </div>
        <label class="sched-field">
          <span>{t("Time (server time)")}</span>
          <input class="input" type="time" value={draft().time} onInput={(event) => patch({ time: event.currentTarget.value })} />
        </label>
        <label class="sched-field">
          <span>{t("Files per run")}</span>
          <input class="input" type="number" min="1" max="500" value={draft().maxFiles} onInput={(event) => patch({ maxFiles: Number(event.currentTarget.value) })} />
        </label>
      </div>
      <Show when={problems().length > 0 && formError() === null}><p class="muted">{problems()[0]}</p></Show>
      <Show when={formError()}><p class="notice bad" role="alert">{formError()}</p></Show>
      <div class="insp-actions">
        <Button variant="primary" disabled={busy() || problems().length > 0} onClick={() => void save()}>{t("Save schedule")}</Button>
        <Button variant="quiet" onClick={closeEditor}>{t("Cancel")}</Button>
      </div>
    </div>
  );

  const renderQuestion = (schedule: Schedule) => (
    <div class="confirm" role="region" aria-label={t("Delete {name}", { name: schedule.name })}>
      <p>{t("Delete “{name}”? Jobs it already queued keep running.", { name: schedule.name })}</p>
      <Show when={rowError()}><p class="notice bad" role="alert">{rowError()}</p></Show>
      <div class="insp-actions">
        <Button
          variant="primary"
          disabled={busy()}
          onClick={() => void act(() => deleteSchedule(props.api, schedule.id, key), t("Schedule deleted."), t("Unable to delete the schedule."))}
        >
          {t("Confirm delete")}
        </Button>
        <Button variant="quiet" onClick={() => setAsking(null)}>{t("Cancel")}</Button>
      </div>
    </div>
  );

  const renderRow = (schedule: Schedule) => (
    <li class="sched-row" classList={{ paused: !schedule.enabled }} data-id={schedule.id}>
      <div class="sched-line">
        <label class="sched-switch">
          <input type="checkbox" checked={schedule.enabled} disabled={busy() || schedule.action !== "verify"} aria-label={schedule.enabled ? t("Pause {name}", { name: schedule.name }) : t("Turn on {name}", { name: schedule.name })} onChange={() => toggle(schedule)} />
        </label>
        <div class="sched-main">
          <b class="sched-name">{schedule.name}</b>
          <span class="muted sched-target">{describeTarget(schedule)}</span>
        </div>
        <div class="sched-when">
          <span>{`${describeDays(schedule.days)} · ${schedule.time}`}</span>
          <span class="muted">{describeNext(schedule)}</span>
        </div>
        <div class="sched-last">
          <Show when={schedule.last_run_at} fallback={<span class="muted">{t("Never run")}</span>}>
            <span>{describeLast(schedule)}</span>
            <Show when={schedule.last_job_id}>
              {(job) => <a href={`/jobs/${encodeURIComponent(job())}`}>{t("Open job")}</a>}
            </Show>
          </Show>
          <span class="muted">{tn("{count} file matches now", "{count} files match now", schedule.matching)}</span>
        </div>
        <div class="sched-actions">
          <Button variant="quiet" disabled={busy() || schedule.action !== "verify"} onClick={() => { fresh(); void act(() => runScheduleNow(props.api, schedule.id, key), t("Run started. The result is on the schedule."), t("Unable to run the schedule.")); }}>{t("Run now")}</Button>
          <Button variant="quiet" disabled={busy()} onClick={() => openEdit(schedule)}>{t("Edit")}</Button>
          <Button variant="quiet" disabled={busy()} onClick={() => ask(schedule.id, "delete")}>{t("Delete")}</Button>
        </div>
      </div>
      <Show when={asking()?.id === schedule.id}>{renderQuestion(schedule)}</Show>
      <Show when={rowError() && asking() === null}><p class="notice bad" role="alert">{rowError()}</p></Show>
    </li>
  );

  return (
    <section class="schedules" aria-labelledby="schedules-title" onKeyDown={onKeyDown}>
      <header class="sched-head">
        <h1 id="schedules-title">{t("Schedules")}</h1>
        <Button variant="primary" ref={(element) => { newButton = element; }} aria-expanded={editing() !== null} onClick={openNew}>{t("New schedule")} <Kbd>N</Kbd></Button>
      </header>
      <p class="muted sched-lede">{t("Check the files that match a filter, on the days and time you choose. Times are the server's.")}</p>
      <Show when={notice()}><p class="notice ok" role="status">{notice()}</p></Show>
      <Show when={editing()}>{renderEditor()}</Show>
      <Show when={phase() !== "loading"} fallback={<p class="muted">{t("Loading schedules")}</p>}>
        <Show when={phase() === "ready"} fallback={
          <div class="notice bad" role="alert">
            <p>{phase() === "session" ? t(SESSION_EXPIRED) : t("Unable to load schedules")}</p>
            <Show when={phase() === "error"}><Button onClick={() => { setPhase("loading"); void load(); }}>{t("Retry")}</Button></Show>
          </div>
        }>
          <Show
            when={rows().length > 0}
            fallback={
              <div class="empty">
                <b>{t("No schedules yet")}</b>
                <span>{t("A schedule only reads files: it checks them and changes nothing. Preparing a file is always your decision.")}</span>
              </div>
            }
          >
            <ul class="plain sched-list" aria-label={t("Schedules")}>
              <For each={rows()}>{renderRow}</For>
            </ul>
          </Show>
        </Show>
      </Show>
    </section>
  );
};

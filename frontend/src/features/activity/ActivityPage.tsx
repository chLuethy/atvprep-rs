import { createEffect, createMemo, createSignal, For, onSettled, Show, type Component } from "solid-js";
import { Button, Icon, ProgressBar } from "../../ui";
import { FilterBar, NoMatches, type Facet } from "../../ui/FilterBar";
import { StatusChip } from "../../ui";
import { JobDetail } from "./JobDetail";
import { periodLibraryFacets } from "./PeriodLibrary";
import { flash } from "../../lib/flash";
import { isTyping } from "../../lib/keys";
import { uniqueRowKeys } from "../../lib/rowKeys";
import { clearJobs, loadJobCounts, loadJobs, type Api, type ClearStatus, type JobRow } from "./loaders";
import { isJobFiltered, JOB_ACTIONS, JOB_STATUSES, JOB_TRIGGERS, jobFilterSpec, revealActive, sameJobFilter, type JobCounts, type JobFilter, type JobSort } from "./filters";
import { createFacetFilter } from "../../lib/facetFilter";
import { N, t, tn } from "../../lib/i18n";
import { autoDismiss } from "../../lib/autoDismiss";
import { useClock } from "../../lib/clock";
import { SINGLE_COLUMN_MAX } from "../../lib/fitBottom";
import { baseName } from "../files/model";
import { actionLabel, isActive, jobPercent, jobProgress, jobTitle, jobTone, partitionJobs, runningFor } from "./model";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");

/** What "Clear finished" can hide; one choice may cover several server statuses. */
const CLEARABLE: ReadonlyArray<{ id: string; label: string; statuses: ClearStatus[] }> = [
  { id: "succeeded", label: N("Succeeded"), statuses: ["succeeded"] },
  { id: "failed", label: N("Failed"), statuses: ["failed", "partial_success"] },
  { id: "cancelled", label: N("Cancelled"), statuses: ["cancelled"] },
];

type Phase = "loading" | "ready" | "error" | "session";

/**
 * The API contract has no naturally unique job key: only link complete, unique candidates. Only fields that
 * cannot change while a job runs are part of it (not the total or the subject), so a row keeps its focus.
 */
const jobKey = (job: JobRow): string | undefined => job.requested_at && job.action && job.trigger
  ? JSON.stringify([job.requested_at, job.action, job.trigger]) : undefined;

const Row: Component<{ job: JobRow; current: boolean; flagged: boolean; onOpen: (id: string) => void }> = (props) => {
  const tone = () => jobTone(props.job.status);
  const now = useClock();
  const liveText = () => {
    const percent = jobPercent(props.job);
    const time = runningFor(props.job.started_at, now());
    if (percent === undefined) return time ? t("running for {time}", { time }) : "";
    return time ? t("{percent}% · running for {time}", { percent, time }) : `${percent}%`;
  };
  let item: HTMLLIElement | undefined;
  createEffect(() => ({ job: props.job, flagged: props.flagged }), ({ flagged }) => { if (flagged && item) flash(item); });
  return (
    <li ref={item} class="job-row" aria-current={props.current ? "true" : undefined}>
      <a
        class="job-link"
        href={`/jobs/${encodeURIComponent(props.job.id)}`}
        data-id={props.job.id}
        onClick={(event) => { event.preventDefault(); props.onOpen(props.job.id); }}
      >
        <span class="job-title">{jobTitle(props.job)}</span>
        <Show when={props.job.matched_path}>
          {(path) => <span class="job-match muted" title={path()}>{t("Match: {file}", { file: baseName(path()).title })}</span>}
        </Show>
        <span class="job-meta muted">{`${actionLabel(props.job.action)} · ${jobProgress(props.job)}`}</span>
        <Show when={props.job.status === "running"}>
          <span class="job-live">
            <Show when={jobPercent(props.job) !== undefined}>
              <ProgressBar value={jobPercent(props.job) ?? 0} track={jobKey(props.job) ?? props.job.id} label={t("Progress of {title}", { title: jobTitle(props.job) })} />
            </Show>
            <span class="muted num">{liveText()}</span>
          </span>
        </Show>
      </a>
      <StatusChip kind={tone().kind} icon={tone().icon} live={tone().live}>{tone().label}</StatusChip>
    </li>
  );
};

/** What the worker is doing and has done, newest first, with the open job's detail beside it. */
export const ActivityPage: Component<{
  api: Api;
  activeId?: string | undefined;
  /** The filter the address asks for; it is followed when it changes (Back). */
  filter?: JobFilter | undefined;
  /**
   * A filter chosen here, for the address. `replace` is set for a search, so typing does not add a history
   * entry per pause; a chosen value or order does add one, so Back undoes it.
   */
  onFilter?: (filter: JobFilter, how: { replace: boolean }) => void;
  onOpen: (id: string | undefined) => void;
}> = (props) => {
  const [rows, setRows] = createSignal<JobRow[]>([]);
  const rowKey = createMemo(() => uniqueRowKeys(rows(), jobKey));
  const [nextCursor, setNextCursor] = createSignal<string | undefined>();
  const [phase, setPhase] = createSignal<Phase>("loading");
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [moreProblem, setMoreProblem] = createSignal<string | null>(null);
  // The period and library facets need the libraries by name; the filter's spec reaches them lazily.
  const facet = createFacetFilter(
    jobFilterSpec({ value: () => periodLibrary.value(), apply: (value, next) => periodLibrary.apply(value, next) }),
    { filter: () => props.filter, onFilter: (next, how) => props.onFilter?.(next, how) },
  );
  const filter = facet.filter;
  const changeFilter = facet.change;
  const [counts, setCounts] = createSignal<JobCounts | null>(null);
  const [notice, setNotice] = createSignal<string | null>(null);
  autoDismiss(notice, () => setNotice(null));
  const [clearOpen, setClearOpen] = createSignal(false);
  const [clearPick, setClearPick] = createSignal<string[]>(["succeeded", "failed"]);
  const [clearBusy, setClearBusy] = createSignal(false);
  const [clearError, setClearError] = createSignal<string | null>(null);
  let clearKey = crypto.randomUUID();
  let clearInFlight = false;
  let clearTrigger: HTMLButtonElement | undefined;

  const openClear = () => { clearKey = crypto.randomUUID(); setClearError(null); setClearOpen(true); };
  const closeClear = () => { setClearOpen(false); queueMicrotask(() => clearTrigger?.focus()); };
  const togglePick = (id: string) => {
    clearKey = crypto.randomUUID();
    setClearPick((current) => current.includes(id) ? current.filter((known) => known !== id) : [...current, id]);
  };
  const submitClear = async () => {
    if (clearInFlight || clearPick().length === 0) return;
    clearInFlight = true;
    setClearBusy(true);
    setClearError(null);
    const statuses = CLEARABLE.filter((entry) => clearPick().includes(entry.id)).flatMap((entry) => entry.statuses);
    const result = await clearJobs(props.api, statuses, clearKey);
    clearInFlight = false;
    setClearBusy(false);
    if (disposed) return;
    if (!result.ok) { setClearError(result.session ? t(SESSION_EXPIRED) : t("Unable to clear jobs. Try again.")); return; }
    setClearOpen(false);
    setNotice(tn("Cleared {count} job. They stay in the audit trail.", "Cleared {count} jobs. They stay in the audit trail.", result.cleared));
    if (props.activeId) props.onOpen(undefined);
    await load(true);
    await refreshCounts();
  };

  let generation = 0;
  let pages = 1;
  // Jobs to mark as their rows are built: new ones and those whose status changed in the last refresh.
  let flagged = new Set<string>();
  let moreInFlight = false;
  let listLoading = false;
  let disposed = false;
  let refreshFlight: { generation: number; promise: Promise<void> } | undefined;
  const load = async (reset: boolean, cursor?: string, asked = filter(), api = props.api) => {
    if (disposed || (!reset && moreInFlight)) return;
    listLoading = true;
    moreInFlight = !reset;
    setMoreProblem(null);
    const mine = ++generation;
    flagged = new Set();
    if (reset) { setPhase("loading"); pages = 1; } else setLoadingMore(true);
    const page = await loadJobs(api, { filter: asked, cursor });
    if (mine !== generation) return;
    moreInFlight = false;
    listLoading = false;
    setLoadingMore(false);
    if (page.kind !== "ok") {
      if (!reset && rows().length > 0) { setMoreProblem(page.kind === "session" ? t(SESSION_EXPIRED) : t("Unable to load jobs")); return; }
      setPhase(page.kind === "session" ? "session" : "error");
      return;
    }
    if (!reset) pages += 1;
    setRows(reset ? page.items : [...rows(), ...page.items]);
    setNextCursor(page.nextCursor);
    setPhase("ready");
  };

  /** Replace all visible pages; UI keys stay distinct even when response IDs change. */
  const readFreshRows = async () => {
    const mine = generation;
    const asked = filter();
    const count = pages;
    let cursor: string | undefined;
    const fresh: JobRow[] = [];
    for (let index = 0; index < count; index += 1) {
      const page = await loadJobs(props.api, { filter: asked, cursor });
      if (mine !== generation || page.kind !== "ok") return;
      fresh.push(...page.items);
      cursor = page.nextCursor;
      if (!cursor) break;
    }
    const oldKey = rowKey();
    const newKey = uniqueRowKeys(fresh, jobKey);
    const before = new Map(rows().map((row) => [oldKey(row), row]));
    flagged = new Set(fresh.filter((row) => before.get(newKey(row))?.status !== row.status).map((row) => row.id));
    const focused = document.activeElement;
    const keepFocus = focused instanceof HTMLElement && !!focused.closest(".job-row");
    setRows(fresh);
    if (keepFocus) onSettled(() => {
      if (focused.isConnected && document.activeElement === document.body) focused.focus();
    });
    setNextCursor(cursor);
    await refreshCounts();
  };
  // Active and relative polls share the same read; loading more and filter changes take precedence.
  const refresh = (): Promise<void> => {
    if (disposed || listLoading) return Promise.resolve();
    if (refreshFlight?.generation === generation) return refreshFlight.promise;
    const flight = { generation, promise: readFreshRows() };
    refreshFlight = flight;
    void flight.promise.finally(() => { if (refreshFlight === flight) refreshFlight = undefined; });
    return flight.promise;
  };
  onSettled(() => () => { disposed = true; generation += 1; });
  /** The facet counters, "X of Y" and the hidden active jobs; without an answer none of them show. */
  let countsGeneration = 0;
  const refreshCounts = async (asked = filter(), api = props.api) => {
    const mine = ++countsGeneration;
    const answer = await loadJobCounts(api, asked);
    if (!disposed && mine === countsGeneration && sameJobFilter(asked, filter())) setCounts(answer);
  };

  const periodLibrary = periodLibraryFacets({ get api() { return props.api; }, filter, counts: () => counts()?.facets ?? null, onChange: (next) => changeFilter(next) });
  const showHidden = () => {
    const known = counts();
    if (!known) return;
    const next = revealActive(filter(), known);
    if (next.q !== filter().q) facet.replaceAll(next);
    else changeFilter(next);
  };
  const facets = (): Facet[] => [
    { key: "status", title: t("Status"), logic: "or", options: JOB_STATUSES.map((entry) => ({ value: entry.token, label: t(entry.label), urgent: entry.urgent })) },
    { key: "action", title: t("Action"), logic: "or", options: JOB_ACTIONS.map((entry) => ({ value: entry.token, label: t(entry.label) })) },
    triggerFacet(),
    ...periodLibrary.facets(),
  ];
  /** Auslöser: by hand, retries and the group of every schedule, each schedule by name; a chosen one since deleted stays. */
  const triggerFacet = (): Facet => {
    const known = (counts()?.schedules ?? []).map((schedule) => ({ value: schedule.id, label: schedule.name }));
    const gone = (filter().schedule ?? []).filter((id) => !known.some((option) => option.value === id)).map((id) => ({ value: id, label: t("Deleted schedule") }));
    return {
      key: "trigger", title: t("Trigger"), logic: "or",
      options: JOB_TRIGGERS.map((entry) => ({ value: entry.token, label: t(entry.label) })),
      group: { value: "schedule", key: "schedule", searchable: true, options: [...known, ...gone] },
    };
  };
  const summaryText = () => {
    const answer = counts();
    return answer ? tn("{matched} of {count} job", "{matched} of {count} jobs", answer.total, { matched: answer.matched }) : null;
  };
  const hiddenActive = () => counts()?.hiddenActive ?? 0;
  // Nothing found and nothing filtered out: no job has run yet, as opposed to a filter without hits.
  const noJobsYet = () => !isJobFiltered(filter()) || counts()?.total === 0;

  type OpenedJob = { requestedId: string | undefined; key: string | undefined; identity: object };
  const opened = createMemo((previous?: OpenedJob): OpenedJob => {
    const requestedId = props.activeId;
    const direct = rows().find((row) => row.id === requestedId);
    const candidate = direct ? rowKey()(direct) : undefined;
    const remembered = requestedId === previous?.requestedId ? previous?.key : undefined;
    const key = direct ? (typeof candidate === "string" ? candidate : undefined)
      : remembered !== undefined && rows().some((row) => rowKey()(row) === remembered) ? remembered : undefined;
    // The first match of a deep link does not replace its already usable detail instance.
    // A lost or ambiguous match, however, must not later attach to a different job.
    const same = previous && ((requestedId === previous.requestedId && (previous.key === undefined || previous.key === key))
      || (key !== undefined && key === previous.key));
    return { requestedId, key, identity: same ? previous.identity : {} };
  });
  const isCurrent = (job: JobRow) => props.activeId === job.id || (!!props.activeId && opened().key !== undefined && rowKey()(job) === opened().key);
  const currentJobId = () => rows().find((row) => row.id === props.activeId)?.id ?? rows().find(isCurrent)?.id ?? props.activeId;

  createEffect(() => ({ filter: filter(), api: props.api }), (asked) => { void load(true, undefined, asked.filter, asked.api); void refreshCounts(asked.filter, asked.api); });
  const grouped = createMemo(() => partitionJobs(rows()));
  // Hidden active jobs keep the refresh going too, so the hint follows them.
  const anyActive = createMemo(() => rows().some(isActive) || hiddenActive() > 0);
  createEffect(anyActive, (active) => {
    if (!active) return;
    // Once a second, so the bar and the clock move together; a slow answer is not stacked on.
    let reading = false;
    const timer = window.setInterval(() => {
      if (document.hidden || reading) return;
      reading = true;
      void refresh().finally(() => { reading = false; });
    }, 1000);
    return () => window.clearInterval(timer);
  });
  // A relative period ("Today", "Last 24 hours") moves with the clock: once a minute the list and the counters
  // are asked for with fresh bounds, so a page left open past midnight does not keep yesterday's.
  const relativePeriod = () => { const period = filter().period; return !!period && "preset" in period; };
  createEffect(relativePeriod, (relative) => {
    if (!relative) return;
    let reading = false;
    const timer = window.setInterval(() => {
      if (reading || anyActive()) return;
      reading = true;
      void refresh().finally(() => { reading = false; });
    }, 60 * 1000);
    return () => window.clearInterval(timer);
  });

  // Up to SINGLE_COLUMN_MAX the detail stands under the list: bring it into view when a job is opened.
  let side: HTMLElement | undefined;
  const revealDetail = () => {
    if (window.matchMedia?.(`(max-width: ${SINGLE_COLUMN_MAX}px)`).matches) side?.scrollIntoView?.({ block: "start" });
  };

  const move = (delta: number, from: string | undefined) => {
    const list = [...grouped().active, ...grouped().history];
    const index = from ? list.findIndex((row) => row.id === from) : -1;
    const next = list[Math.min(list.length - 1, Math.max(0, index + delta))];
    if (!next) return;
    props.onOpen(next.id);
    queueMicrotask(() => document.querySelector<HTMLElement>(`a.job-link[data-id="${next.id}"]`)?.focus());
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (isTyping(event.target)) return;
    const link = (event.target as HTMLElement).closest?.("a.job-link") as HTMLElement | null;
    const from = link?.dataset.id ?? props.activeId;
    if (event.key === "ArrowDown" || event.key === "j") { event.preventDefault(); move(1, from); }
    else if (event.key === "ArrowUp" || event.key === "k") { event.preventDefault(); move(-1, from); }
  };

  // The list arrives as an accessor: called with a value, this would build the whole section again
  // whenever any job changed. Read through the accessor, only the rows that changed are touched.
  const group = (label: string, list: () => JobRow[]) => (
    <Show when={list().length > 0}>
      <section class="job-group" role="region" aria-label={label}>
        <h2>{label}</h2>
        <ul class="plain">
          <For each={list()} keyed={(job) => rowKey()(job)}>{(job) => <Row job={job()} current={isCurrent(job())} flagged={flagged.has(job().id)} onOpen={(id) => { setNotice(null); props.onOpen(id); revealDetail(); }} />}</For>
        </ul>
      </section>
    </Show>
  );

  return (
    <div class="library activity" onKeyDown={onKeyDown}>
      <section class="lib-main" aria-label={t("Activity")}>
        <div class="lib-bar">
          <div class="lib-bar-top">
            <h1>{t("Activity")}</h1>
            <Button variant="quiet" ref={(element) => { clearTrigger = element; }} aria-expanded={clearOpen() ? "true" : "false"} onClick={() => (clearOpen() ? closeClear() : openClear())}>{t("Clear finished")}</Button>
          </div>
          <Show when={clearOpen()}>
            <div class="confirm" role="region" aria-label={t("Clear finished jobs")} onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); closeClear(); } }}>
              <p>{t("Hide finished jobs from this list. Their records stay in the audit trail.")}</p>
              <div class="chips">
                <For each={CLEARABLE}>{(entry) => (
                  <label class="pick"><input type="checkbox" checked={clearPick().includes(entry.id)} onChange={() => togglePick(entry.id)} />{t(entry.label)}</label>
                )}</For>
              </div>
              <Show when={clearError()}><p class="notice bad" role="alert">{clearError()}</p></Show>
              <div class="insp-actions">
                <Button variant="primary" disabled={clearBusy() || clearPick().length === 0} onClick={() => void submitClear()}>{t("Confirm clear")}</Button>
                <Button variant="quiet" onClick={closeClear}>{t("Keep")}</Button>
              </div>
            </div>
          </Show>
          <FilterBar
            label={t("Filter jobs")}
            facets={facets()}
            value={facet.bar().value}
            counts={counts()?.facets ?? null}
            summary={summaryText()}
            countText={(count) => tn("{count} job", "{count} jobs", count)}
            dirty={facet.bar().dirty}
            onChange={facet.bar().onChange}
            onReset={facet.bar().onReset}
            quick={facet.bar().quick}
            onQuick={facet.bar().onQuick}
            search={(
              <div class="search">
                <Icon name="search" />
                <input
                  class="input"
                  type="search"
                  placeholder={t("Search jobs")}
                  aria-label={t("Search jobs")}
                  value={facet.search()}
                  onInput={(event) => facet.onSearchInput(event.currentTarget.value)}
                />
              </div>
            )}
            tools={(
              <select class="select" aria-label={t("Order")} value={filter().sort} onChange={(event) => changeFilter({ ...filter(), sort: event.currentTarget.value as JobSort })}>
                <option value="newest">{t("Newest first")}</option>
                <option value="oldest">{t("Oldest first")}</option>
              </select>
            )}
          />
        </div>
        <Show when={notice()}><p class="notice ok lib-notice" role="status">{notice()}</p></Show>
        <div class="lib-scroll">
          <Show when={phase() !== "loading"} fallback={<p class="muted pad">{t("Loading jobs")}</p>}>
            <Show when={phase() === "ready"} fallback={
              <div class="notice bad lib-notice" role="alert">
                <p>{phase() === "session" ? t(SESSION_EXPIRED) : t("Unable to load jobs")}</p>
                <Show when={phase() === "error"}>
                  <Button onClick={() => void load(true)}>{t("Retry")}</Button>
                  {/* A link with a filter the server refuses fails every time; resetting gets out of it. */}
                  <Show when={isJobFiltered(filter())}><Button variant="quiet" onClick={facet.reset}>{t("Reset filters")}</Button></Show>
                </Show>
              </div>
            }>
              <Show when={hiddenActive() > 0}>
                <p class="job-hidden muted" role="status" aria-label={t("Hidden active jobs")}>
                  {tn("{count} active job hidden", "{count} active jobs hidden", hiddenActive())}
                  <button type="button" class="fb-link" onClick={showHidden}>{t("Show them")}</button>
                </p>
              </Show>
              <Show when={rows().length > 0} fallback={(
                <Show when={noJobsYet()} fallback={(
                  <NoMatches title={t("No jobs match the current filters")} facets={facets()} value={facet.bar().value} search={filter().q} onReset={facet.reset} />
                )}>
                  <div class="empty"><b>{t("No jobs yet")}</b>{t("Select a file in Files and press V to verify it, or F to preview preparing it. The job shows up here.")}</div>
                </Show>
              )}>
                {group(t("Active"), () => grouped().active)}
                {group(t("History"), () => grouped().history)}
              </Show>
              <Show when={nextCursor()}>
                <div class="more"><Show when={moreProblem()}><p class="notice bad" role="alert">{moreProblem()}</p></Show><Button disabled={loadingMore()} onClick={() => void load(false, nextCursor())}>{t("Load more jobs")}</Button></div>
              </Show>
            </Show>
          </Show>
        </div>
      </section>
      <aside class="lib-side" ref={(element) => { side = element; }} aria-label={t("Job details")}>
        <Show when={props.activeId} fallback={<div class="empty"><b>{t("Select a job to see what it did")}</b>{t("Use the arrow keys to move through the list.")}</div>}>
          {(_id) => <JobDetail api={props.api} jobId={currentJobId()!} identity={opened().identity} onChanged={() => void refresh()} onRetried={(newId) => { void refresh(); setNotice(t("Retry queued. Showing the new job.")); props.onOpen(newId); }} onRaised={(newId) => { void refresh(); setNotice(t("Preparation queued. Showing the new job.")); props.onOpen(newId); }} />}
        </Show>
      </aside>
    </div>
  );
};

import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type Component } from "solid-js";
import { Button, Icon, ProgressBar } from "../../ui";
import { FilterBar, type Facet, type FilterValue } from "../../ui/FilterBar";
import { StatusChip } from "../../ui";
import { JobDetail } from "./JobDetail";
import { flash } from "../../lib/flash";
import { isTyping } from "../../lib/keys";
import { reuseUnchanged } from "../../lib/stable";
import { clearJobs, loadJobCounts, loadJobs, type Api, type ClearStatus, type JobRow } from "./loaders";
import { EMPTY_JOB_FILTER, isJobFiltered, JOB_ACTIONS, JOB_STATUSES, revealActive, sameJobFilter, type JobCounts, type JobFilter, type JobSort } from "./filters";
import { N, t, tn } from "../../lib/i18n";
import { autoDismiss } from "../../lib/autoDismiss";
import { useClock } from "../../lib/clock";
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

/** Ids change with every response, so the open job is recognised by what it is rather than by id. */
const fingerprint = (job: JobRow) => [job.requested_at, job.action, job.trigger, job.total_count, job.subject].join("|");
const jobKey = (job: JobRow) => `${job.requested_at}|${job.action}|${job.trigger}`;

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
  // A job that turned up, or whose status changed, while the list was open is marked once. A row is
  // built anew whenever its job changes, so this runs for each of those, and not for a first load.
  onMount(() => { if (props.flagged && item) flash(item); });
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
              <ProgressBar value={jobPercent(props.job) ?? 0} track={jobKey(props.job)} label={t("Progress of {title}", { title: jobTitle(props.job) })} />
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
  const [nextCursor, setNextCursor] = createSignal<string | undefined>();
  const [phase, setPhase] = createSignal<Phase>("loading");
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [filter, setFilter] = createSignal<JobFilter>(props.filter ?? EMPTY_JOB_FILTER, { equals: sameJobFilter });
  /** What the search field shows; the filter takes it after a short pause. */
  const [search, setSearch] = createSignal(filter().q);
  const [counts, setCounts] = createSignal<JobCounts | null>(null);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(searchTimer));
  const [notice, setNotice] = createSignal<string | null>(null);
  autoDismiss(notice, () => setNotice(null));
  const [clearOpen, setClearOpen] = createSignal(false);
  const [clearPick, setClearPick] = createSignal<string[]>(["succeeded", "failed"]);
  const [clearBusy, setClearBusy] = createSignal(false);
  const [clearError, setClearError] = createSignal<string | null>(null);
  let clearKey = crypto.randomUUID();
  let clearTrigger: HTMLButtonElement | undefined;

  const openClear = () => { clearKey = crypto.randomUUID(); setClearError(null); setClearOpen(true); };
  const closeClear = () => { setClearOpen(false); queueMicrotask(() => clearTrigger?.focus()); };
  const togglePick = (id: string) => {
    clearKey = crypto.randomUUID();
    setClearPick(clearPick().includes(id) ? clearPick().filter((known) => known !== id) : [...clearPick(), id]);
  };
  const submitClear = async () => {
    if (clearBusy() || clearPick().length === 0) return;
    setClearBusy(true);
    setClearError(null);
    const statuses = CLEARABLE.filter((entry) => clearPick().includes(entry.id)).flatMap((entry) => entry.statuses);
    const result = await clearJobs(props.api, statuses, clearKey);
    setClearBusy(false);
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
  const load = async (reset: boolean, cursor?: string) => {
    const mine = ++generation;
    flagged = new Set();
    if (reset) { setPhase("loading"); pages = 1; } else setLoadingMore(true);
    const page = await loadJobs(props.api, { filter: filter(), cursor });
    if (mine !== generation) return;
    setLoadingMore(false);
    if (page.kind !== "ok") {
      setPhase(page.kind === "session" ? "session" : "error");
      return;
    }
    if (!reset) pages += 1;
    setRows(reset ? page.items : [...rows(), ...page.items]);
    setNextCursor(page.nextCursor);
    setPhase("ready");
  };

  /**
   * Reload every page loaded so far and replace the list. The server issues fresh ids on every
   * response, so rows cannot be merged by id; replacing keeps the list exactly as long as it was.
   */
  const refresh = async () => {
    const mine = generation;
    let cursor: string | undefined;
    const fresh: JobRow[] = [];
    for (let index = 0; index < pages; index += 1) {
      const page = await loadJobs(props.api, { filter: filter(), cursor });
      if (mine !== generation || page.kind !== "ok") return;
      fresh.push(...page.items);
      cursor = page.nextCursor;
      if (!cursor) break;
    }
    const before = new Map(rows().map((row) => [jobKey(row), row]));
    flagged = new Set(fresh.filter((row) => before.get(jobKey(row))?.status !== row.status).map(jobKey));
    setRows(reuseUnchanged(fresh, rows(), jobKey));
    setNextCursor(cursor);
    await refreshCounts();
  };
  /** The facet counters, "X of Y" and the hidden active jobs; without an answer none of them show. */
  const refreshCounts = async () => {
    const asked = filter();
    const answer = await loadJobCounts(props.api, asked);
    if (sameJobFilter(asked, filter())) setCounts(answer);
  };

  const changeFilter = (next: JobFilter, replace = false) => {
    if (sameJobFilter(next, filter())) return;
    setFilter(next);
    props.onFilter?.(next, { replace });
  };
  const clearFilters = () => {
    clearTimeout(searchTimer);
    setSearch("");
    changeFilter({ ...EMPTY_JOB_FILTER, sort: filter().sort });
  };
  const onSearchInput = (value: string) => {
    setSearch(value);
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => changeFilter({ ...filter(), q: value.trim() }, true), 250);
  };
  const showHidden = () => {
    const known = counts();
    if (!known) return;
    const next = revealActive(filter(), known);
    if (next.q !== filter().q) { clearTimeout(searchTimer); setSearch(next.q); }
    changeFilter(next);
  };
  const facets = (): Facet[] => [
    { key: "status", title: t("Status"), logic: "or", options: JOB_STATUSES.map((entry) => ({ value: entry.token, label: t(entry.label), urgent: entry.urgent })) },
    { key: "action", title: t("Action"), logic: "or", options: JOB_ACTIONS.map((entry) => ({ value: entry.token, label: t(entry.label) })) },
  ];
  const facetValue = (): FilterValue => ({ status: filter().status, action: filter().action });
  const onFacets = (value: FilterValue) => changeFilter({ ...filter(), status: [...(value.status ?? [])], action: [...(value.action ?? [])] });
  const summaryText = () => {
    const answer = counts();
    return answer ? tn("{matched} of {count} job", "{matched} of {count} jobs", answer.total, { matched: answer.matched }) : null;
  };
  const hiddenActive = () => counts()?.hiddenActive ?? 0;
  // Nothing found and nothing filtered out: no job has run yet, as opposed to a filter without hits.
  const noJobsYet = () => !isJobFiltered(filter()) || counts()?.total === 0;

  const [openedFingerprint, setOpenedFingerprint] = createSignal<string | undefined>();
  createEffect(() => {
    const open = props.activeId ? rows().find((row) => row.id === props.activeId) : undefined;
    if (open) setOpenedFingerprint(fingerprint(open));
    else if (!props.activeId) setOpenedFingerprint(undefined);
  });
  const isCurrent = (job: JobRow) => props.activeId === job.id || (!!props.activeId && openedFingerprint() === fingerprint(job));

  // The address changed from outside (Back, a link): follow it, search field included.
  createEffect(on(() => props.filter, (next) => {
    const wanted = next ?? EMPTY_JOB_FILTER;
    if (sameJobFilter(wanted, filter())) return;
    clearTimeout(searchTimer);
    setSearch(wanted.q);
    setFilter(wanted);
  }, { defer: true }));
  createEffect(on(filter, () => { void load(true); void refreshCounts(); }));
  const grouped = createMemo(() => partitionJobs(rows()));
  // Hidden active jobs keep the refresh going too, so the hint follows them.
  const anyActive = () => rows().some(isActive) || hiddenActive() > 0;
  createEffect(() => {
    if (!anyActive()) return;
    // Once a second, so the bar and the clock move together; a slow answer is not stacked on.
    let reading = false;
    const timer = window.setInterval(() => {
      if (document.hidden || reading) return;
      reading = true;
      void refresh().finally(() => { reading = false; });
    }, 1000);
    onCleanup(() => window.clearInterval(timer));
  });

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
          <For each={list()}>{(job) => <Row job={job} current={isCurrent(job)} flagged={flagged.has(jobKey(job))} onOpen={(id) => { setNotice(null); props.onOpen(id); }} />}</For>
        </ul>
      </section>
    </Show>
  );

  return (
    <div class="library activity" data-pane={props.activeId ? "detail" : "list"} onKeyDown={onKeyDown}>
      <section class="lib-main" aria-label={t("Activity")}>
        <div class="lib-bar">
          <div class="lib-bar-top">
            <h1>{t("Activity")}</h1>
            <Button variant="quiet" ref={(element) => { clearTrigger = element; }} aria-expanded={clearOpen()} onClick={() => (clearOpen() ? closeClear() : openClear())}>{t("Clear finished")}</Button>
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
            value={facetValue()}
            counts={counts()?.facets ?? null}
            summary={summaryText()}
            dirty={isJobFiltered(filter())}
            onChange={onFacets}
            onReset={clearFilters}
            search={(
              <div class="search">
                <Icon name="search" />
                <input
                  class="input"
                  type="search"
                  placeholder={t("Search jobs")}
                  aria-label={t("Search jobs")}
                  value={search()}
                  onInput={(event) => onSearchInput(event.currentTarget.value)}
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
                <Show when={phase() === "error"}><Button onClick={() => void load(true)}>{t("Retry")}</Button></Show>
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
                  <div class="empty">
                    <b>{t("No jobs match the current filters")}</b>
                    {t("Change the filters or clear the search.")}
                    <div><Button onClick={clearFilters}>{t("Reset filters")}</Button></div>
                  </div>
                )}>
                  <div class="empty"><b>{t("No jobs yet")}</b>{t("Select a file in Files and press V to verify it, or F to preview preparing it. The job shows up here.")}</div>
                </Show>
              )}>
                {group(t("Active"), () => grouped().active)}
                {group(t("History"), () => grouped().history)}
              </Show>
              <Show when={nextCursor()}>
                <div class="more"><Button disabled={loadingMore()} onClick={() => void load(false, nextCursor())}>{t("Load more jobs")}</Button></div>
              </Show>
            </Show>
          </Show>
        </div>
      </section>
      <aside class="lib-side" aria-label={t("Job details")}>
        <Button variant="quiet" class="back-to-list" onClick={() => props.onOpen(undefined)}>{t("Back to list")}</Button>
        <Show when={props.activeId} fallback={<div class="empty"><b>{t("Select a job to see what it did")}</b>{t("Use the arrow keys to move through the list.")}</div>}>
          {(id) => <JobDetail api={props.api} jobId={id()} onChanged={() => void refresh()} onRetried={(newId) => { void refresh(); setNotice(t("Retry queued. Showing the new job.")); props.onOpen(newId); }} onRaised={(newId) => { void refresh(); setNotice(t("Preparation queued. Showing the new job.")); props.onOpen(newId); }} />}
        </Show>
      </aside>
    </div>
  );
};

import { createEffect, createMemo, createResource, createSignal, For, Index, Match, onCleanup, Show, Switch, untrack, type Accessor, type Component } from "solid-js";
import { keepIfSame, loadedFor, reuseUnchanged } from "../../lib/stable";
import { Button, Icon, ProgressBar, StatusChip } from "../../ui";
import { formatDateTime } from "../../lib/time";
import { useClock } from "../../lib/clock";
import { N, t } from "../../lib/i18n";
import { baseName } from "../files/model";
import { loadItems, loadJob, mutateJob, type Api, type ItemRow, type JobRow, type OneJob } from "./loaders";
import { LogViewer } from "./LogViewer";
import { RaiseEncode } from "./RaiseEncode";
import { actionLabel, canCancel, canRetry, describeDetail, formatElapsed, isActive, itemTone, jobPercent, jobProgress, jobTitle, jobTone, runningFor } from "./model";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");
const FAILED_ITEM = new Set(["failed", "stale_input", "interrupted"]);
type Operation = "cancel" | "retry";

/**
 * Keeps the previous object when a read of the same job finds only renamed ids. Opening another job
 * is not such a read: its entries can look identical to the last job's, and must not borrow its ids.
 */
const sameRead = () => {
  let last: string | undefined;
  return <T,>(id: string, next: T, previous: T | undefined): T => {
    const same = last === id;
    last = id;
    return same ? keepIfSame(next, previous) : next;
  };
};
const formatDate = (iso: string | null) => formatDateTime(iso);
const itemName = (item: ItemRow) => baseName(item.display_path ?? "").title || t("Item {position}", { position: item.position });

const ItemLog: Component<{ api: Api; itemId: string }> = (props) => {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="item-log">
      <Button variant="quiet" aria-expanded={open()} onClick={() => setOpen((value) => !value)}>{t("Log")}</Button>
      <Show when={open()}><LogViewer api={props.api} itemId={props.itemId} /></Show>
    </div>
  );
};

/** One job: the outcome, why anything failed, its items, and the actions the server will accept. */
export const JobDetail: Component<{
  api: Api;
  jobId: string;
  onChanged?: () => void;
  onRetried?: (newJobId: string) => void;
  onRaised?: (newJobId: string) => void;
}> = (props) => {
  const keepJob = sameRead();
  const keepItems = sameRead();
  // A read that finds only renamed ids keeps the previous object, so an unchanged job changes nothing on screen.
  const [job, { refetch: refetchJob }] = createResource<OneJob, string>(
    () => props.jobId,
    async (id, info) => keepJob(id, await loadJob(props.api, id), info.value),
  );
  const [firstPage, { refetch: refetchItems }] = createResource<Awaited<ReturnType<typeof loadItems>>, string>(
    () => props.jobId,
    async (id, info) => keepItems(id, await loadItems(props.api, id), info.value),
  );
  const jobReady = loadedFor(job, () => props.jobId);
  const itemsReady = loadedFor(firstPage, () => props.jobId);
  const [items, setItems] = createSignal<ItemRow[]>([]);
  const [nextCursor, setNextCursor] = createSignal<string | undefined>();
  const [moreLoading, setMoreLoading] = createSignal(false);
  const [moreError, setMoreError] = createSignal(false);
  const [moreSession, setMoreSession] = createSignal(false);

  const [pending, setPending] = createSignal<Operation | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  const [message, setMessage] = createSignal<string | null>(null);
  const [failure, setFailure] = createSignal<"update" | "session" | null>(null);
  let idempotencyKey: string | null = null;
  const triggers: Partial<Record<Operation, HTMLButtonElement>> = {};
  let confirmButton: HTMLButtonElement | undefined;
  let lastOperation: Operation | null = null;

  let itemsOf: string | undefined;
  createEffect(() => {
    const page = firstPage();
    if (page?.kind === "ok") {
      // Rows are matched by position: one that did not change stays the same object, and a row that
      // did keeps its id, so only what moved is touched and an open log stays open. Only within one
      // job: entry 0 of another job looks the same but is a different entry with its own log.
      const job = untrack(() => props.jobId);
      const sameJob = itemsOf === job;
      itemsOf = job;
      setItems((existing) => reuseUnchanged(page.items, sameJob ? existing : [], (row) => String(row.position)));
      setNextCursor(page.nextCursor);
    }
  });
  createEffect(() => {
    void props.jobId;
    setPending(null);
    setMessage(null);
    setFailure(null);
    setMoreError(false);
    setMoreSession(false);
    idempotencyKey = null;
  });
  createEffect(() => {
    const operation = pending();
    if (operation) {
      lastOperation = operation;
      queueMicrotask(() => confirmButton?.focus());
    } else if (lastOperation) {
      const trigger = triggers[lastOperation];
      lastOperation = null;
      queueMicrotask(() => trigger?.focus());
    }
  });

  const current = () => {
    const state = job();
    return state?.kind === "ok" ? state.data : undefined;
  };
  // An active job changes underneath us: reload it (and its first page of items) while it runs.
  createEffect(() => {
    const active = current() && isActive(current()!);
    if (!active) return;
    let reading = false;
    const timer = window.setInterval(() => {
      if (document.hidden || reading) return;
      reading = true;
      void Promise.allSettled([refetchJob(), refetchItems()]).finally(() => { reading = false; });
    }, 1000);
    onCleanup(() => window.clearInterval(timer));
  });

  const open = (operation: Operation, trigger: HTMLButtonElement) => {
    triggers[operation] = trigger;
    idempotencyKey ??= crypto.randomUUID();
    setMessage(null);
    setFailure(null);
    setPending(operation);
  };
  const close = () => {
    setPending(null);
    idempotencyKey = null;
  };
  const submit = async () => {
    const operation = pending();
    if (!operation || submitting()) return;
    setSubmitting(true);
    setFailure(null);
    try {
      const result = await mutateJob(props.api, props.jobId, operation, idempotencyKey ?? crypto.randomUUID());
      if (result.ok) {
        setMessage(operation === "cancel" ? t("Job cancellation requested") : t("Job retry requested"));
        close();
        void refetchJob();
        void refetchItems();
        props.onChanged?.();
        if (operation === "retry" && result.id) props.onRetried?.(result.id);
      } else if (result.session) {
        setFailure("session");
        close();
      } else {
        setFailure("update");
      }
    } finally {
      setSubmitting(false);
    }
  };
  const loadMore = async () => {
    const cursor = nextCursor();
    if (!cursor || moreLoading()) return;
    setMoreLoading(true);
    setMoreError(false);
    setMoreSession(false);
    try {
      const page = await loadItems(props.api, props.jobId, cursor);
      if (page.kind === "ok") {
        setItems((existing) => [...existing, ...page.items.filter((next) => !existing.some((known) => known.id === next.id))]);
        setNextCursor(page.nextCursor);
      } else if (page.kind === "session") setMoreSession(true);
      else setMoreError(true);
    } finally {
      setMoreLoading(false);
    }
  };

  const failedItems = () => items().filter((item) => FAILED_ITEM.has(item.status));

  const jobFailure = createMemo(() => {
    const state = job();
    return !state || state.kind === "error" ? "error" : state.kind === "session" ? "session" : undefined;
  });
  const data = createMemo<JobRow | undefined>(() => {
    const state = job();
    return state?.kind === "ok" ? state.data : undefined;
  });

  const renderJob = () => (
    <Switch>
      <Match when={jobFailure() === "error"}>
        <div class="notice bad" role="alert"><p>{t("Unable to load job")}</p><Button onClick={() => void refetchJob()}>{t("Retry")}</Button></div>
      </Match>
      <Match when={jobFailure() === "session"}>
        <p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p>
      </Match>
      <Match when={data()}>{(d) => renderData(d)}</Match>
    </Switch>
  );

  const renderData = (d: Accessor<JobRow>) => {
    const tone = () => jobTone(d().status);
    const now = useClock();
    const since = () => (d().status === "running" ? runningFor(d().started_at, now()) : "");
    return (
      <>
        <header class="insp-head">
          <h2>{jobTitle(d())}</h2>
          <div class="job-summary" role="group" aria-label={t("Job summary")}>
            <StatusChip kind={tone().kind} icon={tone().icon} live={tone().live}>{tone().label}</StatusChip>
            <span>{jobProgress(d())}</span>
            <Show when={jobPercent(d()) !== undefined}>
              <span class="job-progress">
                <ProgressBar value={jobPercent(d()) ?? 0} track={`job:${props.jobId}`} label={t("Job progress")} />
                <span class="num">{`${jobPercent(d())}%`}</span>
              </span>
            </Show>
          </div>
          <dl class="facts">
            <div><dt>{t("Action")}</dt><dd>{actionLabel(d().action)}</dd></div>
            <Show when={d().trigger === "retry"}><div><dt>{t("Origin")}</dt><dd>{t("Retry of an earlier job")}</dd></div></Show>
            <Show when={d().schedule}>{(schedule) => (
              <div><dt>{t("Origin")}</dt><dd>{schedule().name === null ? t("A deleted schedule") : t("Schedule „{name}“", { name: schedule().name ?? "" })}</dd></div>
            )}</Show>
            <Show when={d().requested_at}><div><dt>{t("Requested")}</dt><dd>{formatDate(d().requested_at)}</dd></div></Show>
            <Show when={formatElapsed(d().started_at, d().finished_at)}><div><dt>{t("Took")}</dt><dd>{formatElapsed(d().started_at, d().finished_at)}</dd></div></Show>
            <Show when={since()}><div><dt>{t("Running for")}</dt><dd class="num">{since()}</dd></div></Show>
          </dl>
          <Show when={d().delete_original}><p class="muted">{t("The original of another container is removed once its new MP4 passes verification.")}</p></Show>
          <div class="insp-actions">
            <Show when={canCancel(d())}>
              <Button ref={(element) => { triggers.cancel = element; }} aria-expanded={pending() === "cancel"} onClick={(event) => open("cancel", event.currentTarget)}>{t("Cancel job")}</Button>
            </Show>
            <Show when={canRetry(d())}>
              <Button ref={(element) => { triggers.retry = element; }} aria-expanded={pending() === "retry"} onClick={(event) => open("retry", event.currentTarget)}>{t("Retry failed items")}</Button>
            </Show>
          </div>
          <Show when={pending()}>
            {(operation) => (
              <div class="confirm" role="region" aria-label={operation() === "cancel" ? t("Confirm cancel") : t("Confirm retry")} onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); close(); } }}>
                <p>{operation() === "cancel" ? t("Stop this job? Items that are already running may finish first.") : t("Queue a new job for the failed items? Items that succeeded are left alone.")}</p>
                <div class="insp-actions">
                  <Button variant="primary" disabled={submitting()} ref={(element) => { confirmButton = element; }} onClick={() => void submit()}>{operation() === "cancel" ? t("Confirm cancel") : t("Confirm retry")}</Button>
                  <Button variant="quiet" onClick={close}>{t("Never mind")}</Button>
                </div>
              </div>
            )}
          </Show>
          <Show when={message()}><p class="notice ok" role="status">{message()}</p></Show>
          <Show when={failure() === "update"}><p class="notice bad" role="alert">{t("Unable to update job")}</p></Show>
          <Show when={failure() === "session"}><p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p></Show>
        </header>

        <Show when={(d().status === "failed" || d().status === "partial_success") && itemsReady()}>
          <section class="insp-sec" role="region" aria-label={t("Why it failed")}>
            <h3>{t("Why it failed")}</h3>
            <Show when={failedItems().length > 0} fallback={<p class="muted">{t("The worker recorded no reason. Open an item's log below.")}</p>}>
              <ul class="why">
                <For each={failedItems()}>{(item) => {
                  const itemState = itemTone(item.status);
                  return (
                    <li class="why-item">
                      <StatusChip kind={itemState.kind} icon={itemState.icon} live={itemState.live}>{itemState.label}</StatusChip>
                      <div>
                        <b>{itemName(item)}</b>
                        <Show when={item.error_code}><span class="mono rule">{item.error_code}</span></Show>
                        <p>{item.error_detail ? describeDetail(item.error_detail) : t("No detail was recorded.")}</p>
                      </div>
                    </li>
                  );
                }}</For>
              </ul>
            </Show>
          </section>
        </Show>

        <section class="insp-sec" aria-labelledby="items-title">
          <h3 id="items-title">{t("Items")}</h3>
          <Show when={itemsReady() ? firstPage() : undefined} fallback={<p class="muted">{t("Loading items")}</p>}>
            {(page) => (
              <Show when={page().kind === "ok"} fallback={
                page().kind === "session"
                  ? <p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p>
                  : <div class="notice bad" role="alert"><p>{t("Unable to load items")}</p><Button onClick={() => void refetchItems()}>{t("Retry loading items")}</Button></div>
              }>
                <Show when={items().length > 0} fallback={<p class="muted">{t("No items")}</p>}>
                  <ul class="plain items">
                    <Show when={props.jobId} keyed><Index each={items()}>{(item) => {
                      const itemState = () => itemTone(item().status);
                      const percent = () => Math.max(0, Math.min(100, Math.round(item().progress ?? 0)));
                      // A job of one file already names it in the heading above; repeat it quietly, on one line.
                      const repeatsTitle = () => items().length === 1 && itemName(item()) === jobTitle(d());
                      return (
                        <li class="item-row">
                          <div class="item-line">
                            <span class="mono muted item-pos">{`#${item().position}`}</span>
                            <span class="item-name" classList={{ repeat: repeatsTitle() }} title={repeatsTitle() ? itemName(item()) : undefined}>{itemName(item())}</span>
                            <StatusChip kind={itemState().kind} icon={itemState().icon} live={itemState().live}>{itemState().label}</StatusChip>
                            <Show when={item().attempt > 1 || (item().status === "running" && typeof item().progress === "number")}>
                              <span class="item-progress">
                                <Show when={item().status === "running" && typeof item().progress === "number"}>
                                  <ProgressBar value={percent()} track={`item:${props.jobId}:${item().position}`} label={t("Item {position} progress", { position: item().position })} />
                                  <span class="muted num">{`${percent()}%`}</span>
                                </Show>
                                <Show when={item().attempt > 1}><span class="muted">{t("attempt {attempt}", { attempt: item().attempt })}</span></Show>
                              </span>
                            </Show>
                          </div>
                          <Show when={item().error_code === "encode_not_allowed"}>
                            <p class="notice warn" role="status" aria-label={t("Encoding not allowed")}>{t("Encoding not allowed: the file stays as it was. Prepare it again with a higher encoding allowance.")}</p>
                            <RaiseEncode api={props.api} item={item()} onRaised={(id) => { props.onChanged?.(); if (id) props.onRaised?.(id); }} />
                          </Show>
                          <ItemLog api={props.api} itemId={item().id} />
                        </li>
                      );
                    }}</Index></Show>
                  </ul>
                </Show>
                <Show when={moreSession()}><p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p></Show>
                <Show when={moreError()}><div class="notice bad" role="alert"><p>{t("Unable to load more items")}</p><Button onClick={() => void loadMore()}>{t("Retry")}</Button></div></Show>
                <Show when={nextCursor() && !moreError() && !moreSession()}>
                  <Button disabled={moreLoading()} onClick={() => void loadMore()}>{t("Load more items")}</Button>
                </Show>
              </Show>
            )}
          </Show>
        </section>
      </>
    );
  };

  return (
    <div class="inspector">
      <Show when={jobReady()} fallback={<p class="muted"><Icon name="activity" /> {t("Loading job")}</p>}>{renderJob()}</Show>
    </div>
  );
};

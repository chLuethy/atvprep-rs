import { createEffect, createMemo, createSignal, For, Loading, Match, onSettled, refresh, Show, Switch, type Accessor, type Component } from "solid-js";
import { uniqueRowKeys } from "../../lib/rowKeys";
import { Button, Icon, ProgressBar, StatusChip } from "../../ui";
import { formatDateTime } from "../../lib/time";
import { useClock } from "../../lib/clock";
import { N, t } from "../../lib/i18n";
import { baseName } from "../files/model";
import { parseMediaName, shortName } from "../../lib/mediaName";
import { loadItems, loadJob, mutateJob, type Api, type ItemRow, type JobRow, type Page } from "./loaders";
import { LogViewer } from "./LogViewer";
import { PlannedChange } from "./PlannedChange";
import { RaiseEncode } from "./RaiseEncode";
import { actionLabel, canCancel, canRetry, describeDetail, describeErrorCode, formatElapsed, isActive, isItemFailed, itemDuration, itemTone, jobPercent, jobProgress, jobTitle, jobTone, runningFor, type ItemStatus } from "./model";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");
type Operation = "cancel" | "retry";

const formatDate = (iso: string | null) => formatDateTime(iso);
const fullItemName = (item: ItemRow) => baseName(item.display_path ?? "").title;
const itemName = (item: ItemRow) => (fullItemName(item) ? shortName(parseMediaName(fullItemName(item)), { withGroup: true }) : t("Item {position}", { position: item.position }));

const ItemLog: Component<{ api: Api; itemId: string; status: ItemStatus }> = (props) => {
  const [open, setOpen] = createSignal(false);
  return (
    <div class={["item-log", { open: open() }]}>
      <button type="button" class="log-toggle" aria-expanded={open() ? "true" : "false"} onClick={() => setOpen((value) => !value)}>
        <Icon name="chevron" />{t("Log")}
      </button>
      <Show when={open()}><LogViewer api={props.api} itemId={props.itemId} status={props.status} /></Show>
    </div>
  );
};

/** Why an item failed, in words: what the code means, the worker's own text where it adds to it, the raw code on request. */
const ErrorReason: Component<{ code: string | null | undefined; detail: string | null | undefined }> = (props) => {
  const meaning = () => (props.code ? describeErrorCode(props.code) : undefined);
  const detail = () => (props.detail ? describeDetail(props.detail) : undefined);
  return (
    <>
      <Show when={meaning()}>{(text) => <p>{text()}</p>}</Show>
      <Show when={detail() && detail() !== meaning()}>
        <p>{detail()}</p>
      </Show>
      <Show when={!meaning() && !detail()}><p>{t("No detail was recorded.")}</p></Show>
      <Show when={props.code}>{(code) => (
        <Show when={meaning()} fallback={<span class="mono rule">{code()}</span>}>
          <details class="why-code"><summary>{t("Show code")}</summary><span class="mono rule">{code()}</span></details>
        </Show>
      )}</Show>
    </>
  );
};

/** One job: the outcome, why anything failed, its items, and the actions the server will accept. */
const JobDetailView: Component<{
  api: Api;
  jobId: string;
  onChanged?: () => void;
  onRetried?: (newJobId: string) => void;
  onRaised?: (newJobId: string) => void;
}> = (props) => {
  const job = createMemo(() => loadJob(props.api, props.jobId));
  let disposed = false;
  onSettled(() => () => { disposed = true; });
  let pages = 1;
  const itemPages = createMemo(async (): Promise<Page<ItemRow>> => {
    const api = props.api;
    const jobId = props.jobId;
    const count = pages;
    const items: ItemRow[] = [];
    let cursor: string | undefined;
    for (let index = 0; index < count; index += 1) {
      const page = await loadItems(api, jobId, cursor);
      if (page.kind !== "ok") return page;
      const known = new Set(items.map((item) => item.id));
      items.push(...page.items.filter((item) => !known.has(item.id)));
      cursor = page.nextCursor;
      if (!cursor) break;
    }
    return { kind: "ok", items, nextCursor: cursor };
  }, { loadingValue: undefined });
  const refetchJob = () => refresh(job);
  let itemRefresh: Promise<unknown> | undefined;
  const refetchItems = async () => {
    if (disposed || moreInFlight) return;
    if (itemRefresh) return itemRefresh;
    const request = refresh(itemPages);
    itemRefresh = request;
    try { return await request; }
    finally { if (itemRefresh === request) itemRefresh = undefined; }
  };
  const [items, setItems] = createSignal<ItemRow[]>([]);
  const [nextCursor, setNextCursor] = createSignal<string | undefined>();
  const [moreLoading, setMoreLoading] = createSignal(false);
  const [moreError, setMoreError] = createSignal(false);
  const [moreSession, setMoreSession] = createSignal(false);
  let moreInFlight = false;
  // Positions are assigned sequentially within a job; invalid or duplicate values get no shared UI identity.
  const itemKey = createMemo(() => uniqueRowKeys(items(), (item) =>
    Number.isSafeInteger(item.position) && item.position >= 0 ? String(item.position) : undefined));

  const [pending, setPending] = createSignal<Operation | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  const [message, setMessage] = createSignal<string | null>(null);
  const [failure, setFailure] = createSignal<"update" | "session" | null>(null);
  let idempotencyKey: string | null = null;
  const triggers: Partial<Record<Operation, HTMLButtonElement>> = {};
  let confirmButton: HTMLButtonElement | undefined;
  let lastOperation: Operation | null = null;

  let itemList: HTMLUListElement | undefined;
  createEffect(itemPages, (page) => {
    if (page?.kind === "ok") {
      const focused = document.activeElement;
      const keepFocus = focused instanceof HTMLElement && itemList?.contains(focused);
      setItems(page.items);
      setNextCursor(page.nextCursor);
      // DOM moves can drop browser focus even when the row is preserved.
      if (keepFocus) onSettled(() => {
        if (focused.isConnected && document.activeElement === document.body) focused.focus();
      });
    }
  });
  createEffect(pending, (operation) => {
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
  // An active job keeps being read; switching jobs creates a new instance.
  createEffect(() => !!current() && isActive(current()!), (active) => {
    if (!active) return;
    let reading = false;
    const timer = window.setInterval(() => {
      if (document.hidden || reading) return;
      reading = true;
      void Promise.allSettled([refetchJob(), refetchItems()]).finally(() => { reading = false; });
    }, 1000);
    return () => window.clearInterval(timer);
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
  let submitInFlight = false;
  const submit = async () => {
    const operation = pending();
    if (!operation || submitInFlight) return;
    submitInFlight = true;
    setSubmitting(true);
    setFailure(null);
    try {
      const result = await mutateJob(props.api, props.jobId, operation, idempotencyKey ?? crypto.randomUUID());
      if (disposed) return;
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
      submitInFlight = false;
      setSubmitting(false);
    }
  };
  const loadMore = async () => {
    if (!nextCursor() || moreInFlight) return;
    moreInFlight = true;
    setMoreLoading(true);
    setMoreError(false);
    setMoreSession(false);
    try {
      // A running revalidation determines the current cursor first; polls wait until loading more has finished.
      if (itemRefresh) await itemRefresh;
      if (disposed) return;
      const cursor = nextCursor();
      if (!cursor) return;
      const page = await loadItems(props.api, props.jobId, cursor);
      if (disposed) return;
      if (page.kind === "ok") {
        pages += 1;
        setItems((existing) => [...existing, ...page.items.filter((next) => !existing.some((known) => known.id === next.id))]);
        setNextCursor(page.nextCursor);
      } else if (page.kind === "session") setMoreSession(true);
      else setMoreError(true);
    } finally {
      moreInFlight = false;
      setMoreLoading(false);
    }
  };

  const failedItems = () => items().filter((item) => isItemFailed(item.status));

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
          <dl class="job-facts">
            <div><dt class="label">{t("Action")}</dt><dd>{actionLabel(d().action)}</dd></div>
            <Show when={d().trigger === "retry"}><div><dt class="label">{t("Origin")}</dt><dd>{t("Retry of an earlier job")}</dd></div></Show>
            <Show when={d().schedule}>{(schedule) => (
              <div><dt class="label">{t("Origin")}</dt><dd>{schedule().name === null ? t("A deleted schedule") : t("Schedule „{name}“", { name: schedule().name ?? "" })}</dd></div>
            )}</Show>
            <Show when={d().requested_at}><div><dt class="label">{t("Requested")}</dt><dd>{formatDate(d().requested_at)}</dd></div></Show>
            <Show when={formatElapsed(d().started_at, d().finished_at)}><div><dt class="label">{t("Took")}</dt><dd>{formatElapsed(d().started_at, d().finished_at)}</dd></div></Show>
            <Show when={since()}><div><dt class="label">{t("Running for")}</dt><dd class="num">{since()}</dd></div></Show>
          </dl>
          <Show when={d().delete_original}><p class="muted">{t("The original of another container is removed once its new MP4 passes verification.")}</p></Show>
          <div class="insp-actions">
            <Show when={canCancel(d())}>
              <Button ref={(element) => { triggers.cancel = element; }} aria-expanded={pending() === "cancel" ? "true" : "false"} onClick={(event) => open("cancel", event.currentTarget)}>{t("Cancel job")}</Button>
            </Show>
            <Show when={canRetry(d())}>
              <Button ref={(element) => { triggers.retry = element; }} aria-expanded={pending() === "retry" ? "true" : "false"} onClick={(event) => open("retry", event.currentTarget)}>{t("Retry failed items")}</Button>
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

        <Show when={(d().status === "failed" || d().status === "partial_success") && itemPages() !== undefined}>
          <section class="insp-sec" role="region" aria-label={t("Why it failed")}>
            <h3>{t("Why it failed")}</h3>
            <Show when={failedItems().length > 0} fallback={<p class="muted">{t("The worker recorded no reason. Open an item's log below.")}</p>}>
              <ul class="why">
                <For each={failedItems()}>{(item) => {
                  const itemState = () => itemTone(item.status);
                  return (
                    <li class="why-item">
                      <StatusChip kind={itemState().kind} icon={itemState().icon} live={itemState().live}>{itemState().label}</StatusChip>
                      <div>
                        <b>{itemName(item)}</b>
                        <ErrorReason code={item.error_code} detail={item.error_detail} />
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
          <Show when={itemPages()} fallback={<p class="muted">{t("Loading items")}</p>}>
            {(page) => (
              <Show when={page().kind === "ok"} fallback={
                page().kind === "session"
                  ? <p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p>
                  : <div class="notice bad" role="alert"><p>{t("Unable to load items")}</p><Button onClick={() => void refetchItems()}>{t("Retry loading items")}</Button></div>
              }>
                <Show when={items().length > 0} fallback={<p class="muted">{t("No items")}</p>}>
                  <ul class="plain items" ref={(element) => { itemList = element; }}>
                    <For each={items()} keyed={(item) => itemKey()(item)}>{(item) => {
                      const itemState = () => itemTone(item().status);
                      const percent = () => Math.max(0, Math.min(100, Math.round(item().progress ?? 0)));
                      // A job of one file already names it in the heading above; repeat it quietly, on one line.
                      const repeatsTitle = () => items().length === 1 && itemName(item()) === jobTitle(d());
                      return (
                        <li class="item-row">
                          <div class="item-line">
                            <span class="mono muted item-pos">{`#${item().position}`}</span>
                            <span class={["item-name", { repeat: repeatsTitle() }]} title={fullItemName(item()) || undefined}>{itemName(item())}</span>
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
                            <Show when={itemDuration(item(), now())}>{(duration) => <span class="item-duration muted num">{duration()}</span>}</Show>
                          </div>
                          <PlannedChange change={item().planned_change} steps={item().planned_steps} />
                          <Show when={item().error_code === "encode_not_allowed"}>
                            <p class="notice warn" role="status" aria-label={t("Encoding not allowed")}>{t("Encoding not allowed: the file stays as it was. Prepare it again with a higher encoding allowance.")}</p>
                            <RaiseEncode api={props.api} item={item()} onRaised={(id) => { props.onChanged?.(); if (id) props.onRaised?.(id); }} />
                          </Show>
                          <ItemLog api={props.api} itemId={item().id} status={item().status} />
                        </li>
                      );
                    }}</For>
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
      <Loading fallback={<p class="muted"><Icon name="activity" /> {t("Loading job")}</p>}>{renderJob()}</Loading>
    </div>
  );
};

export const JobDetail: Component<Parameters<typeof JobDetailView>[0] & { identity?: string | object | undefined }> = (props) => (
  <Show when={props.identity ?? props.jobId} keyed>{(_identity) => <JobDetailView {...props} />}</Show>
);

import { createEffect, createResource, createSignal, For, on, Show, type Component } from "solid-js";
import { Button } from "../../ui";
import { N, t } from "../../lib/i18n";
import { describeSidecar, formatSize } from "./model";
import { loadSidecars, loadSidecarsPage, type Api, type Sidecar } from "./loaders";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");

/** Sidecar files next to the media file, loaded page by page. */
export const Sidecars: Component<{ api: Api; mediaId: string; count: number; version?: number }> = (props) => {
  const [state, { refetch }] = createResource(() => props.mediaId, (mediaId) => loadSidecars(props.api, mediaId));
  // What happens to a sidecar depends on the newest inspection: read the list again once a job ends,
  // or a finished re-inspection still shows "re-inspect first".
  createEffect(on(() => props.version, () => { void refetch(); }, { defer: true }));
  const [items, setItems] = createSignal<Sidecar[]>([]);
  const [nextCursor, setNextCursor] = createSignal<string | null>(null);
  const [pageLoading, setPageLoading] = createSignal(false);
  const [pageError, setPageError] = createSignal(false);
  const [pageSession, setPageSession] = createSignal(false);

  createEffect(() => {
    const value = state();
    if (value?.kind === "ok") {
      setItems(value.items);
      setNextCursor(value.nextCursor);
      setPageError(false);
      setPageSession(false);
    }
  });

  const loadMore = async () => {
    const cursor = nextCursor();
    if (!cursor || pageLoading()) return;
    setPageLoading(true);
    setPageError(false);
    setPageSession(false);
    try {
      const page = await loadSidecarsPage(props.api, props.mediaId, cursor);
      if (page.kind === "session") setPageSession(true);
      else if (page.kind === "error") setPageError(true);
      else {
        const known = new Set(items().map((item) => item.id));
        setItems((current) => [...current, ...page.items.filter((item) => !known.has(item.id))]);
        setNextCursor(page.nextCursor);
      }
    } finally {
      setPageLoading(false);
    }
  };

  return (
    <section class="insp-sec" aria-labelledby="sidecars-title">
      <h3 id="sidecars-title">{t("Sidecars")}</h3>
      <Show when={!state.loading} fallback={<p class="muted">{t("Loading sidecars")}</p>}>
        <Show when={state()}>
          {(current) => (
            <Show
              when={current().kind === "ok"}
              fallback={
                <Show
                  when={current().kind === "session"}
                  fallback={<div class="notice bad" role="alert"><p>{t("Unable to load sidecars")}</p><Button onClick={() => refetch()}>{t("Retry")}</Button></div>}
                >
                  <p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p>
                </Show>
              }
            >
              <Show
                when={items().length > 0}
                fallback={<p class="muted">{props.count > 0 ? t("{count} sidecars available", { count: props.count }) : t("No sidecars")}</p>}
              >
                <ul class="plain" aria-label={t("Sidecar files")}>
                  <For each={items()}>{(sidecar) => (
                    <li class="sidecar">
                      <span>{sidecar.name}</span> <span class="muted">{formatSize(sidecar.size)}</span>
                      <Show when={describeSidecar(sidecar.state, sidecar.language)}>{(text) => <span class="muted sidecar-state" classList={{ pending: sidecar.state === "pending" }}>{text()}</span>}</Show>
                    </li>
                  )}</For>
                </ul>
              </Show>
              <Show when={pageSession()}><p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p></Show>
              <Show when={pageError()}>
                <div class="notice bad" role="alert"><p>{t("Unable to load more sidecars")}</p><Button onClick={() => void loadMore()}>{t("Retry")}</Button></div>
              </Show>
              <Show when={nextCursor() && !pageError() && !pageSession()}>
                <Button disabled={pageLoading()} onClick={() => void loadMore()}>{pageLoading() ? t("Loading more sidecars") : t("Load more sidecars")}</Button>
              </Show>
            </Show>
          )}
        </Show>
      </Show>
    </section>
  );
};

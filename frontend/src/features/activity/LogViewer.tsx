import { createEffect, createMemo, refresh, Show, type Component } from "solid-js";
import { Button } from "../../ui";
import { N, t } from "../../lib/i18n";
import { loadLog, type Api } from "./loaders";
import { isItemTerminal, type ItemStatus } from "./model";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");

/**
 * The tail of one item's log as plain text. Polls every three seconds while the item can still
 * change, and reads it one last time when the item ends.
 */
export const LogViewer: Component<{ api: Api; itemId: string; status: ItemStatus }> = (props) => {
  const ended = createMemo(() => isItemTerminal(props.status));
  const itemId = createMemo(() => props.itemId);
  const state = createMemo(() => loadLog(props.api, itemId()), { loadingValue: undefined });
  const refetch = () => refresh(state);

  // A growing log stays at its end for a reader who is at the end, and stays put for one who scrolled up.
  let box: HTMLPreElement | undefined;
  let following = true;
  const atEnd = () => !box || box.scrollHeight - box.scrollTop - box.clientHeight < 24;
  createEffect(() => state()?.data?.log_tail, () => { if (box && following) box.scrollTop = box.scrollHeight; });

  createEffect(ended, (isEnded, wasEnded) => { if (isEnded && wasEnded === false) void refetch(); });
  createEffect(ended, (isEnded) => {
    if (isEnded) return;
    const timer = window.setInterval(() => { if (!document.hidden) void refetch(); }, 3000);
    return () => window.clearInterval(timer);
  });

  const download = () => {
    const text = state()?.data?.log_tail ?? "";
    const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `${props.itemId}.log.txt`;
    anchor.click();
    if (typeof URL.revokeObjectURL === "function") window.setTimeout(() => URL.revokeObjectURL(url), 0);
  };

  return (
    <div class="log">
      <div class="log-tools">
        <Button variant="quiet" disabled={!state()?.data} onClick={download}>{t("Download log")}</Button>
      </div>
      <Show when={state()} fallback={<p class="muted">{t("Loading log")}</p>}>
        {(current) => (
          <Show
            when={current().kind !== "session"}
            fallback={<p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p>}
          >
            <Show
              when={current().kind === "ok"}
              fallback={<div class="notice bad" role="alert"><p>{t("Unable to load log")}</p><Button onClick={() => void refetch()}>{t("Retry loading log")}</Button></div>}
            >
              <pre ref={(element) => { box = element; following = true; }} onScroll={() => { following = atEnd(); }} class="raw" aria-label={t("Plain text log")}>{current().data?.log_tail}</pre>
            </Show>
          </Show>
        )}
      </Show>
    </div>
  );
};

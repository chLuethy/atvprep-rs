import { createEffect, createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { Button } from "../../ui";
import { formatDateTime } from "../../lib/time";
import { N, t } from "../../lib/i18n";
import { loadRoots, requestScan, type Api, type RootRow } from "./loaders";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");
const POLL_MS = 4000;

const isActive = (root: RootRow) => root.latest_scan?.status === "queued" || root.latest_scan?.status === "running";

/** One short phrase per library: what its latest scan is doing or did. */
function describe(root: RootRow): string {
  const scan = root.latest_scan;
  if (!scan) return t("never scanned");
  switch (scan.status) {
    case "queued": return t("waiting to start");
    case "running": return t("scanning");
    case "succeeded": return formatDateTime(scan.finished_at) ? t("scanned {date}", { date: formatDateTime(scan.finished_at) }) : t("scanned");
    case "failed": return t("last scan failed");
    case "cancelled": return t("last scan cancelled");
    case "interrupted": return t("last scan was interrupted");
    default: return t("scanned");
  }
}

/**
 * Start a scan of every library by hand and follow it. A scan finds new and changed files, which
 * is the only way a file added to the library shows up before the next scheduled scan.
 */
export const ScanControl: Component<{ api: Api; onFinished?: () => void }> = (props) => {
  const [roots, setRoots] = createSignal<RootRow[] | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  const keys = new Map<string, string>();

  const busy = () => submitting() || (roots() ?? []).some(isActive);
  let wasActive = false;

  const refresh = async () => {
    const result = await loadRoots(props.api);
    if (result.kind === "ok") {
      const active = result.items.some(isActive);
      setRoots(result.items);
      setProblem((current) => (current === t("Unable to load libraries") || current === t(SESSION_EXPIRED) ? null : current));
      if (wasActive && !active) props.onFinished?.();
      wasActive = active;
    } else if (roots() === null && result.kind === "error") {
      // An expired session is the page's message to give; saying it twice would only be noise.
      setProblem(t("Unable to load libraries"));
    }
  };

  onMount(() => void refresh());
  createEffect(() => {
    if (!(roots() ?? []).some(isActive)) return;
    const timer = window.setInterval(() => { if (!document.hidden) void refresh(); }, POLL_MS);
    onCleanup(() => window.clearInterval(timer));
  });

  const scan = async () => {
    if (busy()) return;
    setSubmitting(true);
    setProblem(null);
    let failure: string | null = null;
    for (const root of roots() ?? []) {
      if (!keys.has(root.id)) keys.set(root.id, crypto.randomUUID());
      const result = await requestScan(props.api, root.id, keys.get(root.id) ?? crypto.randomUUID());
      if (!result.ok) {
        failure = result.session ? t(SESSION_EXPIRED) : t("Unable to queue the scan. Try again.");
        break;
      }
      keys.delete(root.id);
    }
    setSubmitting(false);
    if (failure) setProblem(failure);
    await refresh();
  };

  const label = () => (busy() ? t("Scanning") : (roots()?.length ?? 0) > 1 ? t("Scan libraries") : t("Scan library"));

  return (
    <div class="scan-control">
      <Show when={(roots() ?? []).length > 0}>
        <Button disabled={busy()} onClick={() => void scan()}>{label()}</Button>
      </Show>
      <Show when={(roots() ?? []).length > 0}>
        <p class="muted scan-status" role="status">
          <For each={roots() ?? []}>{(root, index) => <>{index() > 0 ? " · " : ""}{`${root.name}: ${describe(root)}`}</>}</For>
        </p>
      </Show>
      <Show when={roots() !== null && roots()?.length === 0}>
        <p class="muted">{t("No library is set up. List its folders under roots in the configuration; the worker registers them when it starts.")}</p>
      </Show>
      <Show when={problem()}><p class="notice bad" role="alert">{problem()}</p></Show>
    </div>
  );
};

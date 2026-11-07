import { createSignal, For, Show, type Component } from "solid-js";
import { Button } from "../../ui";
import { N, t, tn } from "../../lib/i18n";
import { formatSize } from "../../lib/size";
import { removeOrphans, type Api } from "../scan/loaders";

export type OrphanHit = {
  root_id: string;
  name: string;
  count: number;
  bytes: number;
  files: Array<{ relative_path: string; size: number; modified_at: string | null }>;
};

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");

/**
 * Leftover partial rewrites a scan found in one library, with a confirmed way to remove them. The
 * confirmation names count and size; the server deletes only when those still match.
 */
export const Orphans: Component<{ api: Api; hit: OrphanHit; onChanged: () => void; onDone: (message: string) => void }> = (props) => {
  const [confirming, setConfirming] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  let key = crypto.randomUUID();
  let trigger: HTMLButtonElement | undefined;

  const open = () => { key = crypto.randomUUID(); setProblem(null); setConfirming(true); };
  const close = () => { setConfirming(false); queueMicrotask(() => trigger?.focus()); };

  const submit = async () => {
    if (busy()) return;
    setBusy(true);
    setProblem(null);
    const result = await removeOrphans(props.api, props.hit.root_id, props.hit.count, props.hit.bytes, key);
    setBusy(false);
    if (result.kind === "ok") {
      setConfirming(false);
      props.onDone(tn("Removed {count} leftover file ({size}).", "Removed {count} leftover files ({size}).", result.removed, { size: formatSize(result.bytes) }));
    } else if (result.kind === "changed") {
      key = crypto.randomUUID();
      setProblem(t("The leftovers changed since the scan. Check the new numbers and try again."));
      props.onChanged();
    } else {
      key = crypto.randomUUID();
      setProblem(result.kind === "session" ? t(SESSION_EXPIRED) : t("Unable to remove the leftovers. Try again."));
    }
  };

  const more = () => props.hit.count - props.hit.files.length;

  return (
    <div class="orphans">
      <p>{tn("{count} leftover file in {library} ({size})", "{count} leftover files in {library} ({size})", props.hit.count, { library: props.hit.name, size: formatSize(props.hit.bytes) })}</p>
      <ul class="plain muted">
        <For each={props.hit.files}>{(file) => <li>{`${file.relative_path} · ${formatSize(file.size)}`}</li>}</For>
        <Show when={more() > 0}><li>{t("and {count} more", { count: more() })}</li></Show>
      </ul>
      <Button variant="quiet" ref={(element) => { trigger = element; }} aria-expanded={confirming()} onClick={() => (confirming() ? close() : open())}>{t("Remove leftovers")}</Button>
      <Show when={confirming()}>
        <div class="confirm" role="region" aria-label={t("Confirm removing leftovers")} onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); close(); } }}>
          <p>{tn("Delete {count} file, {size} in total?", "Delete {count} files, {size} in total?", props.hit.count, { size: formatSize(props.hit.bytes) })}</p>
          <p class="muted">{t("Only leftover partial rewrites are deleted. Your videos stay.")}</p>
          <div class="insp-actions">
            <Button variant="primary" disabled={busy()} onClick={() => void submit()}>{t("Delete leftovers")}</Button>
            <Button variant="quiet" onClick={close}>{t("Keep")}</Button>
          </div>
        </div>
      </Show>
      <Show when={problem()}><p class="notice bad" role="alert">{problem()}</p></Show>
    </div>
  );
};

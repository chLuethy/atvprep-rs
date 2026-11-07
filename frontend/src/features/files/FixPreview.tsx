import { createSignal, For, Show, type Component } from "solid-js";
import { Button, Icon } from "../../ui";
import type { FileRow, JobResult } from "./loaders";
import { expectedChanges, summarizeChanges, type ChangeKind } from "./fixPlan";
import { isMp4Like } from "./convertPlan";
import { prepareConfirm } from "./preparePlan";
import { N, t, tn } from "../../lib/i18n";
import { baseName, estimateMinutes, formatSize, languageName, parseReason, verdictOf } from "./model";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");
const KIND_LABEL: Record<ChangeKind, string> = { patch: N("Patch"), remux: N("Remux"), reencode: N("Re-encode"), none: N("Unchanged") };

/** "with" confirms the re-encode of every file that needs one, for this job only; "without" leaves those files out. */
export type ReencodeChoice = "with" | "without";

/**
 * What Prepare would touch, shown before anything runs. The changes per file are derived from the
 * verify report (see fixPlan.ts), not simulated: the fix itself decides the final result.
 */
export const FixPreview: Component<{
  files: FileRow[];
  skipped: number;
  onQueue: (reencode: ReencodeChoice) => Promise<JobResult>;
  onClose: () => void;
}> = (props) => {
  const [busy, setBusy] = createSignal(false);
  const [failure, setFailure] = createSignal<"queue" | "session" | null>(null);

  const totalBytes = () => props.files.reduce((sum, file) => sum + file.size, 0);
  const largest = () => props.files.reduce((max, file) => Math.max(max, file.size), 0);
  const count = () => props.files.length;
  const confirms = (file: FileRow) => prepareConfirm(file, []);
  /** An MP4 that is only rewritten in place: one sentence for all of them instead of one each. */
  const plain = (file: FileRow) => isMp4Like(file.suffix) && !confirms(file).reencode;

  const reencodeCount = () => props.files.filter((file) => confirms(file).reencode).length;

  const queue = async (choice: ReencodeChoice) => {
    if (busy()) return;
    setBusy(true);
    setFailure(null);
    try {
      const result = await props.onQueue(choice);
      if (!result.ok) setFailure(result.session ? "session" : "queue");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      class="preview"
      role="region"
      aria-label={t("Prepare preview")}
      onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); props.onClose(); } }}
    >
      <Button variant="quiet" onClick={props.onClose}><Icon name="back" /> {t("Back")}</Button>
      <h2>{t("Prepare preview")}</h2>
      <p class="muted">{t("Nothing is written until you confirm.")}</p>
      <dl class="stats">
        <div><dt>{t("Files")}</dt><dd>{count()}</dd></div>
        <div><dt>{t("To rewrite")}</dt><dd>{formatSize(totalBytes())}</dd></div>
        <div><dt>{t("Estimated")}</dt><dd>{t("about {minutes} min", { minutes: estimateMinutes(totalBytes()) })}</dd></div>
      </dl>
      <p class="muted">{t("Temporary space needed: {size} (largest file). A file is swapped in only after its identity is checked again.", { size: formatSize(largest()) })}</p>
      <h3>{t("Expected changes")}</h3>
      <ul class="plain pv-files">
        <For each={props.files}>{(file) => {
          const name = baseName(file.display_path ?? "");
          const verdict = verdictOf(file);
          const changes = expectedChanges((file.compliance_reasons ?? []).map(parseReason));
          const summary = summarizeChanges(changes);
          return (
            <li class="pv-file">
              <b>{name.title || file.kind}</b> <span class="muted">{verdict.label} · {formatSize(file.size)}</span>
              <Show when={!plain(file)}>
                <For each={confirms(file).lines}>{(line) => <p class={confirms(file).reencode ? "notice warn" : "muted"} role={confirms(file).reencode ? "alert" : undefined}>{line}</p>}</For>
              </Show>
              <Show when={(file.pending_sidecars ?? 0) > 0}>
                <ul class="plain changes" aria-label={t("Subtitles to embed")}>
                  <For
                    each={file.embed_sidecars ?? []}
                    fallback={<li class="change remux"><span class="kind">{t("Embed")}</span><span>{tn("Merge {count} external subtitle file into the MP4", "Merge {count} external subtitle files into the MP4", file.pending_sidecars ?? 0)}</span></li>}
                  >{(sidecar) => (
                    <li class="change remux"><span class="kind">{t("Embed")}</span><span>{sidecar.kind === "forced" ? t("{language} forced subtitle from {name}", { language: languageName(sidecar.language), name: sidecar.name }) : sidecar.kind === "sdh" ? t("{language} SDH subtitle from {name}", { language: languageName(sidecar.language), name: sidecar.name }) : t("{language} subtitle from {name}", { language: languageName(sidecar.language), name: sidecar.name })}</span></li>
                  )}</For>
                </ul>
                <p class="muted">{t("The .srt files stay where they are.")}</p>
              </Show>
              <Show
                when={changes.length > 0}
                fallback={<Show when={(file.pending_sidecars ?? 0) === 0 && isMp4Like(file.suffix)}><p class="muted">{t("The server asked for a fix; the verify report lists nothing more specific.")}</p></Show>}
              >
                <ul class="plain changes">
                  <For each={changes}>{(change) => (
                    <li class={`change ${change.kind}`}><span class="kind">{t(KIND_LABEL[change.kind])}</span><span>{change.text}</span></li>
                  )}</For>
                </ul>
                <p class="muted">{summary.remux ? tn("{resolved} of {count} finding resolved, with a remux", "{resolved} of {count} findings resolved, with a remux", changes.length, { resolved: summary.resolved }) : tn("{resolved} of {count} finding resolved", "{resolved} of {count} findings resolved", changes.length, { resolved: summary.resolved })}</p>
              </Show>
            </li>
          );
        }}</For>
      </ul>
      <Show when={props.skipped > 0}>
        <p class="muted">{tn("{count} selected file skipped: already ready, not verified, not fixable, or nothing a fix would change.", "{count} selected files skipped: already ready, not verified, not fixable, or nothing a fix would change.", props.skipped)}</p>
      </Show>
      <Show when={props.files.some(plain)}>
        <p class="muted">{prepareConfirm({ suffix: ".mp4" }, []).lines[0]}</p>
      </Show>
      <p class="muted">{t("Derived from the verify report. The fix itself decides the final result.")}</p>
      <Show when={failure() === "queue"}><p class="notice bad" role="alert">{t("Unable to queue the prepare job. Try again.")}</p></Show>
      <Show when={failure() === "session"}><p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p></Show>
      <Show when={reencodeCount() > 0}>
        <p class="notice warn" role="alert">{tn("{reencode} of {count} file needs a re-encode. That takes longer and replaces the original. It only runs while the setting “delete_original” is on; otherwise the file stays as it is. Choose whether to re-encode it in this job.", "{reencode} of {count} files need a re-encode. That takes longer and replaces the originals. It only runs while the setting “delete_original” is on; otherwise the files stay as they are. Choose whether to re-encode them in this job.", count(), { reencode: reencodeCount() })}</p>
      </Show>
      <div class="insp-actions">
        <Show
          when={reencodeCount() > 0}
          fallback={
            <Button variant="primary" disabled={busy() || count() === 0} onClick={() => void queue("without")}>
              {tn("Prepare {count} file", "Prepare {count} files", count())}
            </Button>
          }
        >
          <Button variant="primary" disabled={busy()} onClick={() => void queue("with")}>{t("With re-encode ({count})", { count: reencodeCount() })}</Button>
          <Button disabled={busy() || reencodeCount() === count()} onClick={() => void queue("without")}>{t("Without re-encode")}</Button>
        </Show>
        <Button variant="quiet" onClick={props.onClose}>{t("Cancel")}</Button>
      </div>
    </div>
  );
};

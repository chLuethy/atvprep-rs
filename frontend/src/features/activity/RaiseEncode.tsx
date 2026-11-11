import { createSignal, Show, type Component } from "solid-js";
import { t } from "../../lib/i18n";
import { Button } from "../../ui";
import { levelsAbove, requiredLevel, type EncodeLevel } from "../../lib/encode";
import { createEncodeChoice, EncodeChoice } from "../files/EncodeChoice";
import { raiseEncode, type Api, type ItemRow } from "./loaders";

/** The levels above the allowance an item ran with; empty when it already had the highest. */
export const higherLevels = (encode: string | null | undefined): EncodeLevel[] => levelsAbove(encode);

/** The level offered first: the one the file's need asks for, else the next one up. */
const preselected = (levels: EncodeLevel[], need: string | null | undefined): EncodeLevel | undefined => {
  const wanted = requiredLevel(need);
  return wanted && levels.includes(wanted) ? wanted : levels[0];
};

/** „Mit höherer Freigabe vorbereiten“: a new job for one refused item, confirmed here and not borrowed from the old one. */
export const RaiseEncode: Component<{ api: Api; item: ItemRow; onRaised: (jobId: string | undefined) => void }> = (props) => {
  const levels = () => higherLevels(props.item.encode);
  // „Audio und Video“ still waits for its own confirmation when the need preselects it.
  const choice = createEncodeChoice(preselected(levels(), props.item.encode_need));
  const [open, setOpen] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [failure, setFailure] = createSignal<"queue" | "session" | null>(null);
  let key = crypto.randomUUID();
  let trigger: HTMLButtonElement | undefined;

  const close = () => {
    setOpen(false);
    key = crypto.randomUUID();
    queueMicrotask(() => trigger?.focus());
  };
  const submit = async () => {
    if (busy() || !choice.ready()) return;
    setBusy(true);
    setFailure(null);
    const level = choice.level();
    // The key follows the level: a changed choice is a different request, a repeated click the same one.
    const result = await raiseEncode(props.api, props.item.id, level, `${key}-${level}`);
    setBusy(false);
    if (result.ok) {
      setOpen(false);
      props.onRaised(result.id);
    } else {
      setFailure(result.session ? "session" : "queue");
    }
  };

  return (
    <Show when={levels().length > 0}>
      <div class="raise-encode">
        <Button ref={(element) => { trigger = element; }} aria-expanded={open()} onClick={() => { setFailure(null); setOpen((value) => !value); }}>{t("Prepare with a higher allowance")}</Button>
        <Show when={open()}>
          <div class="confirm" role="region" aria-label={t("Prepare with a higher allowance")} onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); close(); } }}>
            <p>{t("This queues a new job for this file at the chosen level. The finished job stays as it is.")}</p>
            <EncodeChoice choice={choice} levels={levels()} />
            <Show when={failure() === "queue"}><p class="notice bad" role="alert">{t("Unable to queue the preparation. Try again.")}</p></Show>
            <Show when={failure() === "session"}><p class="notice bad" role="alert">{t("Session expired. Lock and unlock again.")}</p></Show>
            <div class="insp-actions">
              <Button variant="primary" disabled={busy() || !choice.ready()} onClick={() => void submit()}>{t("Confirm")}</Button>
              <Button variant="quiet" onClick={close}>{t("Never mind")}</Button>
            </div>
          </div>
        </Show>
      </div>
    </Show>
  );
};

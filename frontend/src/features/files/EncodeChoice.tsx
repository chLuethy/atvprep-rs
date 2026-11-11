import { createSignal, createUniqueId, For, Show, type Component } from "solid-js";
import type { EncodeLevel } from "../../lib/encode";
import { N, t } from "../../lib/i18n";

export type { EncodeLevel } from "../../lib/encode";

const LEVELS: ReadonlyArray<{ value: EncodeLevel; label: string; hint: string }> = [
  { value: "copy", label: N("Copy only"), hint: N("Streams are only copied. A file that needs encoding stays unchanged.") },
  { value: "audio", label: N("Encode audio"), hint: N("Audio may be encoded, the video is copied. A file that needs a re-encode stays unchanged.") },
  { value: "all", label: N("Encode audio and video"), hint: N("The video may be re-encoded too. That can take hours and cannot be undone on an MP4.") },
];

/** The level and its confirmation, so that the choice reads the same in the preview and the inspector. */
export function createEncodeChoice(initial: EncodeLevel = "audio") {
  const [level, setLevel] = createSignal<EncodeLevel>(initial);
  const [confirmed, setConfirmed] = createSignal(false);
  const [force, setForce] = createSignal(false);
  return {
    level,
    confirmed,
    /** „Neu erstellen erzwingen“: rebuild even a file at the target, still within the level. */
    force,
    setForce,
    /** Whether the job may be queued: the video level needs its own confirmation. */
    ready: () => level() !== "all" || confirmed(),
    choose: (value: EncodeLevel) => { setLevel(value); setConfirmed(false); },
    setConfirmed,
  };
}

export type EncodeChoiceState = ReturnType<typeof createEncodeChoice>;

export const EncodeChoice: Component<{
  choice: EncodeChoiceState;
  /** Only these levels are offered; all of them when left out. */
  levels?: ReadonlyArray<EncodeLevel>;
  /** Offer „Neu erstellen erzwingen“. Only a manual job may; a schedule never does. */
  offerForce?: boolean;
}> = (props) => {
  const name = createUniqueId();
  const offered = () => LEVELS.filter((level) => !props.levels || props.levels.includes(level.value));
  return (
    <fieldset class="encode-choice">
      <legend>{t("Encoding allowance")}</legend>
      <For each={offered()}>{(level) => (
        <label class="encode-level">
          <input type="radio" name={name} value={level.value} checked={props.choice.level() === level.value} onChange={() => props.choice.choose(level.value)} />
          <span><b>{t(level.label)}</b> <span class="muted">{t(level.hint)}</span></span>
        </label>
      )}</For>
      <Show when={props.choice.level() === "all"}>
        <label class="encode-confirm">
          <input type="checkbox" checked={props.choice.confirmed()} onChange={(event) => props.choice.setConfirmed(event.currentTarget.checked)} />
          <span>{t("Yes, the video of these files may be re-encoded.")}</span>
        </label>
      </Show>
      <Show when={props.offerForce}>
        <label class="encode-confirm">
          <input type="checkbox" checked={props.choice.force()} onChange={(event) => props.choice.setForce(event.currentTarget.checked)} />
          <span><b>{t("Force rebuild")}</b> <span class="muted">{t("Builds the file anew even if it is already at the target. It stays bound to the chosen allowance.")}</span></span>
        </label>
      </Show>
    </fieldset>
  );
};

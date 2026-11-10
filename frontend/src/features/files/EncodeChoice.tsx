import { createSignal, createUniqueId, For, Show, type Component } from "solid-js";
import { N, t } from "../../lib/i18n";

/** The Kodierfreigabe of one job: how far Prepare may encode (ADR 0008). */
export type EncodeLevel = "copy" | "audio" | "all";

const LEVELS: ReadonlyArray<{ value: EncodeLevel; label: string; hint: string }> = [
  { value: "copy", label: N("Copy only"), hint: N("Streams are only copied. A file that needs encoding stays unchanged.") },
  { value: "audio", label: N("Encode audio"), hint: N("Audio may be encoded, the video is copied. A file that needs a re-encode stays unchanged.") },
  { value: "all", label: N("Encode audio and video"), hint: N("The video may be re-encoded too. That can take hours and cannot be undone on an MP4.") },
];

/** The level and its confirmation, so that the choice reads the same in the preview and the inspector. */
export function createEncodeChoice() {
  const [level, setLevel] = createSignal<EncodeLevel>("audio");
  const [confirmed, setConfirmed] = createSignal(false);
  return {
    level,
    confirmed,
    /** Whether the job may be queued: the video level needs its own confirmation. */
    ready: () => level() !== "all" || confirmed(),
    choose: (value: EncodeLevel) => { setLevel(value); setConfirmed(false); },
    setConfirmed,
  };
}

export type EncodeChoiceState = ReturnType<typeof createEncodeChoice>;

export const EncodeChoice: Component<{ choice: EncodeChoiceState }> = (props) => {
  const name = createUniqueId();
  return (
    <fieldset class="encode-choice">
      <legend>{t("Encoding allowance")}</legend>
      <For each={LEVELS}>{(level) => (
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
    </fieldset>
  );
};

import { For, type Component } from "solid-js";
import { language, LANGUAGES, setLanguage, t } from "../lib/i18n";

/** English or German; the choice is remembered and applies at once. */
export const LanguageSwitch: Component = () => (
  <div class="seg" role="group" aria-label={t("Language")}>
    <For each={LANGUAGES}>{(choice) => (
      <button type="button" lang={choice.value} aria-pressed={language() === choice.value} onClick={() => setLanguage(choice.value)}>{choice.label}</button>
    )}</For>
  </div>
);

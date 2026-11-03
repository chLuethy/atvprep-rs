import { createSignal, For, onCleanup, onMount, type Component } from "solid-js";
import { N, t } from "../lib/i18n";

export type Theme = "system" | "light" | "dark";

const KEY = "atvprep-theme";
const CHOICES: ReadonlyArray<{ value: Theme; label: string }> = [
  { value: "system", label: N("System") },
  { value: "light", label: N("Light") },
  { value: "dark", label: N("Dark") },
];

/** `system` leaves data-theme unset so the OS preference decides; light and dark force it. */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
}

export function readStoredTheme(): Theme {
  try {
    const stored = window.localStorage.getItem(KEY);
    return stored === "light" || stored === "dark" ? stored : "system";
  } catch {
    return "system";
  }
}

const THEME_EVENT = "atvprep-theme";

/** Apply, remember and announce a theme, so every switch on the page stays in step. */
export function chooseTheme(value: Theme): void {
  applyTheme(value);
  try {
    window.localStorage.setItem(KEY, value);
  } catch {
    // Storage can be blocked; the choice still applies for this session.
  }
  window.dispatchEvent(new CustomEvent<Theme>(THEME_EVENT, { detail: value }));
}

export const ThemeSwitch: Component = () => {
  const [theme, setTheme] = createSignal<Theme>(readStoredTheme());
  applyTheme(theme());
  const choose = (value: Theme) => chooseTheme(value);
  const follow = (event: Event) => setTheme((event as CustomEvent<Theme>).detail);
  onMount(() => window.addEventListener(THEME_EVENT, follow));
  onCleanup(() => window.removeEventListener(THEME_EVENT, follow));
  return (
    <div class="seg" role="group" aria-label={t("Appearance")}>
      <For each={CHOICES}>{(choice) => (
        <button type="button" aria-pressed={theme() === choice.value} onClick={() => choose(choice.value)}>{t(choice.label)}</button>
      )}</For>
    </div>
  );
};

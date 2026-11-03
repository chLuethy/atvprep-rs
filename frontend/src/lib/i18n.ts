import { createSignal } from "solid-js";
import { de } from "../locales/de";

/**
 * The interface text, in English or German. The English text is the key: `t("Verify")` is English as
 * written, and German looks the same words up in the catalogue (src/locales/de.ts), falling back to the
 * English when a text has no entry. A test keeps the catalogue complete.
 *
 * `t` and `tn` read the language signal, so text that is built where a component renders follows a
 * change of language at once. Text kept in a constant is marked with `N("...")` and translated where
 * it is shown, with `t(constant.label)`.
 */
export type Language = "en" | "de";
type Params = Record<string, string | number>;

const KEY = "atvprep.language";
export const LANGUAGES: ReadonlyArray<{ value: Language; label: string }> = [
  { value: "en", label: "English" },
  { value: "de", label: "Deutsch" },
];

function initialLanguage(): Language {
  try {
    const stored = window.localStorage.getItem(KEY);
    if (stored === "en" || stored === "de") return stored;
  } catch {
    // Storage can be blocked; the browser's language still decides.
  }
  return typeof navigator !== "undefined" && (navigator.language ?? "").toLowerCase().startsWith("de") ? "de" : "en";
}

const [current, setCurrent] = createSignal<Language>(initialLanguage());
if (typeof document !== "undefined") document.documentElement.lang = current();

export const language = current;

/** Choose, remember and announce a language. */
export function setLanguage(next: Language): void {
  setCurrent(next);
  if (typeof document !== "undefined") document.documentElement.lang = next;
  try {
    window.localStorage.setItem(KEY, next);
  } catch {
    // The choice still applies for this session.
  }
}

const fill = (template: string, params?: Params) =>
  params ? template.replace(/\{(\w+)\}/g, (whole, name: string) => (name in params ? String(params[name]) : whole)) : template;

/** Mark a text that is kept in a constant and shown later with `t(...)`. It returns the text unchanged. */
export const N = (text: string): string => text;

/** The text in the chosen language, with `{name}` filled from `params`. */
export function t(text: string, params?: Params): string {
  const entry = current() === "de" ? de[text] : undefined;
  return fill(typeof entry === "string" ? entry : text, params);
}

/**
 * A text with a count: `tn("{count} file", "{count} files", n)`. English takes the second form for
 * everything but one; German finds its pair under the English singular.
 */
export function tn(one: string, many: string, count: number, params?: Params): string {
  const entry = current() === "de" ? de[one] : undefined;
  const pair = Array.isArray(entry) ? entry : undefined;
  const template = pair ? (count === 1 ? pair[0] : pair[1]) : count === 1 ? one : many;
  return fill(template, { count, ...params });
}

/** A number with a fixed count of decimals, in the decimal sign of the chosen language (1.5 or 1,5). */
export function formatNumber(value: number, digits: number): string {
  return new Intl.NumberFormat(locale(), { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(value);
}

/** The locale for dates and numbers: the browser's own when it speaks the chosen language, else the language. */
export function locale(): string | undefined {
  const browser = typeof navigator === "undefined" ? "" : navigator.language ?? "";
  return browser.toLowerCase().startsWith(current()) ? undefined : current();
}

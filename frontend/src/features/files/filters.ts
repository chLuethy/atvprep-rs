import { N } from "../../lib/i18n";

/**
 * The library filter. Every entry is something a file can be, and a ticked entry shows those files:
 * unticking "Ready" hides the ready ones and leaves everything else. The server takes the unticked
 * entries as `hide`, a comma separated list in this order.
 */
export type FilterEntry = { token: string; label: string; urgent?: boolean };

/** What the checker decided about a file, plus whether it is still on disk. A file is in exactly one of these for its verdict. */
export const STATE_FILTERS: ReadonlyArray<FilterEntry> = [
  { token: "clean", label: N("Ready") },
  { token: "warning", label: N("Warning") },
  { token: "blocker", label: N("Blocker"), urgent: true },
  { token: "unknown", label: N("Not verified") },
  { token: "probe_failed", label: N("Inspection failed"), urgent: true },
  { token: "missing", label: N("Missing"), urgent: true },
];

/** Things that are true of a file whatever its verdict is, so a file can have several of them. */
export const TRAIT_FILTERS: ReadonlyArray<FilterEntry> = [
  { token: "convert", label: N("To convert") },
  { token: "reencode", label: N("Needs re-encode") },
  { token: "converted", label: N("Converted, original kept") },
];

const ORDER = [...STATE_FILTERS, ...TRAIT_FILTERS].map((entry) => entry.token);

/** A single state the address or a link asks for, or "" for none and for anything unknown. */
export const knownState = (value: string | undefined): string => (value && ORDER.includes(value) ? value : "");

/** The unticked entries as the canonical list: known tokens only, no repeats, always in the same order. */
export function normalizeHidden(value: Iterable<string> | string | undefined): string {
  const wanted = new Set(typeof value === "string" ? value.split(",") : value ?? []);
  return ORDER.filter((token) => wanted.has(token)).join(",");
}

export const hiddenSet = (value: string): ReadonlySet<string> => new Set(value.split(",").filter(Boolean));

/** The list with one entry flipped between shown and hidden. */
export function toggleHidden(value: string, token: string): string {
  const next = new Set(hiddenSet(value));
  if (next.has(token)) next.delete(token); else next.add(token);
  return normalizeHidden(next);
}

/** The list with every entry of a group shown, or every one of them hidden; the other group stays as it is. */
export function setGroupHidden(value: string, group: ReadonlyArray<FilterEntry>, hidden: boolean): string {
  const next = new Set(hiddenSet(value));
  for (const entry of group) {
    if (hidden) next.add(entry.token); else next.delete(entry.token);
  }
  return normalizeHidden(next);
}

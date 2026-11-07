import { N } from "../../lib/i18n";

/**
 * The library filter has two groups. Status: every entry is something a file can be, and a ticked entry
 * shows those files; unticking "Ready" hides the ready ones. The server takes the unticked entries as
 * `hide`, a comma separated list in this order.
 * Properties overlap, so they work the other way round: nothing ticked filters nothing, and the ticked
 * ones show only the files that have all of them. The server takes them as `has`.
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
  { token: "embed", label: N("Embed open") },
  { token: "converted", label: N("Converted, original kept") },
];

const STATE_ORDER = STATE_FILTERS.map((entry) => entry.token);
const TRAIT_ORDER = TRAIT_FILTERS.map((entry) => entry.token);

/** A single state the address or a link asks for, or "" for none and for anything unknown. */
export const knownState = (value: string | undefined): string => (value && [...STATE_ORDER, ...TRAIT_ORDER].includes(value) ? value : "");

const canonical = (order: string[], value: Iterable<string> | string | undefined): string => {
  const wanted = new Set(typeof value === "string" ? value.split(",") : value ?? []);
  return order.filter((token) => wanted.has(token)).join(",");
};

/**
 * The unticked states as the canonical list: known states only, no repeats, always in the same order.
 * Properties in an old address (`hide=reencode`) are dropped: hiding a property is not a thing any more,
 * and turning it into "only these" would mean the opposite.
 */
export const normalizeHidden = (value: Iterable<string> | string | undefined): string => canonical(STATE_ORDER, value);

export const hiddenSet = (value: string): ReadonlySet<string> => new Set(value.split(",").filter(Boolean));

/** The chosen properties as the canonical list: known properties only, no repeats, always in the same order. */
export const normalizeHas = (value: Iterable<string> | string | undefined): string => canonical(TRAIT_ORDER, value);

export const hasSet = (value: string): ReadonlySet<string> => new Set(value.split(",").filter(Boolean));

/** The list with one entry flipped between shown and hidden. */
export function toggleHidden(value: string, token: string): string {
  const next = new Set(hiddenSet(value));
  if (next.has(token)) next.delete(token); else next.add(token);
  return normalizeHidden(next);
}

/** The chosen properties with one property added or taken out. */
export function toggleHas(value: string, token: string): string {
  const next = new Set(hasSet(value));
  if (next.has(token)) next.delete(token); else next.add(token);
  return normalizeHas(next);
}

/** The list with every state shown, or every one of them hidden. */
export function setGroupHidden(value: string, group: ReadonlyArray<FilterEntry>, hidden: boolean): string {
  const next = new Set(hiddenSet(value));
  for (const entry of group) {
    if (hidden) next.add(entry.token); else next.delete(entry.token);
  }
  return normalizeHidden(next);
}

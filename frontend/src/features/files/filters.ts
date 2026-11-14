import { N } from "../../lib/i18n";

/**
 * The files filter and its address. Status: nothing chosen shows every file, chosen states show the files in
 * any of them (OR); the server takes them as `status`. Properties overlap, so a file must have all chosen ones
 * (AND); the server takes them as `has`. Libraries (`root_id`) and extensions (`suffix`) are OR like the states,
 * `has_sidecar` is yes or no, and the search is `q`. The address uses the server's names.
 */
export type FilterEntry = { token: string; label: string; urgent?: boolean };

/** What the checker decided about a file, plus whether it is still on disk. A file is in exactly one of these. */
export const STATE_FILTERS: ReadonlyArray<FilterEntry> = [
  { token: "clean", label: N("Ready") },
  { token: "embed_open", label: N("Embedding open") },
  { token: "warning", label: N("Warning") },
  { token: "blocker", label: N("Blocker"), urgent: true },
  { token: "unknown", label: N("Not verified") },
  { token: "probe_failed", label: N("Inspection failed"), urgent: true },
  { token: "missing", label: N("Missing"), urgent: true },
];

/** Things that are true of a file whatever its verdict is, so a file can have several of them. */
export const TRAIT_FILTERS: ReadonlyArray<FilterEntry> = [
  { token: "convert", label: N("To convert") },
  { token: "encode_audio", label: N("Needs audio encoding") },
  { token: "encode_video", label: N("Needs video encoding") },
  { token: "embed", label: N("Subtitles embeddable") },
  { token: "converted", label: N("Converted, original kept") },
];

const STATE_ORDER = STATE_FILTERS.map((entry) => entry.token);
const TRAIT_ORDER = TRAIT_FILTERS.map((entry) => entry.token);

export type FileFilter = {
  /** The search, trimmed; "" for none. */
  q: string;
  /** Chosen states in canonical order; none means every state. */
  status: string[];
  /** Chosen properties in canonical order; a file must have all of them. */
  has: string[];
  /** Chosen libraries (their ids), sorted; none means every library. */
  root: string[];
  /** Chosen extensions as stored (".mkv"), sorted; none means every extension. */
  suffix: string[];
  /** Only files with (true) or without (false) a subtitle file beside them; null for both. */
  sidecar: boolean | null;
};

export const EMPTY_FILTER: FileFilter = { q: "", status: [], has: [], root: [], suffix: [], sidecar: null };

const list = (value: string | null): string[] => (value ?? "").split(",").filter(Boolean);
/** Known values only, once each, in the order of `order`. */
const canonical = (order: ReadonlyArray<string>, values: Iterable<string>): string[] => {
  const wanted = new Set(values);
  return order.filter((token) => wanted.has(token));
};
/** Values nobody lists in advance (ids, extensions): once each, sorted. */
const sorted = (values: Iterable<string>): string[] => [...new Set(values)].sort();
const sidecarOf = (value: string | null): boolean | null => (value === "true" ? true : value === "false" ? false : null);

/**
 * The filter an address asks for. Old addresses still work: `hide` listed the states to leave out, and a
 * single `status` could also name a property (`status=encode_video`); both mean the same files as before.
 */
export function parseFileFilter(params: URLSearchParams): FileFilter {
  const asked = list(params.get("status"));
  const has = canonical(TRAIT_ORDER, [...list(params.get("has")), ...asked]);
  let status = canonical(STATE_ORDER, asked);
  if (params.has("hide")) {
    const hidden = new Set(list(params.get("hide")));
    const shown = STATE_ORDER.filter((token) => !hidden.has(token));
    status = (status.length > 0 ? status : STATE_ORDER).filter((token) => shown.includes(token));
    // Every state shown is no filter at all.
    if (status.length === STATE_ORDER.length) status = [];
  }
  return {
    q: (params.get("q") ?? "").trim(),
    status,
    has,
    root: sorted(list(params.get("root_id"))),
    suffix: sorted(list(params.get("suffix"))),
    sidecar: sidecarOf(params.get("has_sidecar")),
  };
}

/**
 * The canonical query string of a filter, without "?": `status`, `has`, `root_id`, `suffix`, `has_sidecar`,
 * then `q`; "" for no filter.
 */
export function fileFilterQuery(filter: FileFilter): string {
  const parts: string[] = [];
  const add = (key: string, values: string[]) => { if (values.length > 0) parts.push(`${key}=${values.map(encodeURIComponent).join(",")}`); };
  add("status", canonical(STATE_ORDER, filter.status));
  add("has", canonical(TRAIT_ORDER, filter.has));
  add("root_id", sorted(filter.root));
  add("suffix", sorted(filter.suffix));
  if (filter.sidecar !== null) parts.push(`has_sidecar=${filter.sidecar}`);
  if (filter.q.trim()) parts.push(`q=${encodeURIComponent(filter.q.trim())}`);
  return parts.join("&");
}

/** Anything at all narrows the list. */
export const isFiltered = (filter: FileFilter): boolean =>
  filter.q !== "" || filter.status.length > 0 || filter.has.length > 0 || filter.root.length > 0 || filter.suffix.length > 0 || filter.sidecar !== null;

/** Two filters show the same files. */
export const sameFilter = (a: FileFilter, b: FileFilter): boolean => fileFilterQuery(a) === fileFilterQuery(b);

export type SortKey = "title" | "status" | "size" | "added";
/** How the list is ordered; null is the order the files were added, oldest first. */
export type FileSort = { key: SortKey; dir: 1 | -1 } | null;
const SORT_KEYS: ReadonlyArray<SortKey> = ["title", "status", "size", "added"];

/** The sort an address asks for: `sort` and `dir=desc`; anything unknown is the default order. */
export function parseFileSort(params: URLSearchParams): FileSort {
  const key = SORT_KEYS.find((entry) => entry === params.get("sort"));
  const dir = params.get("dir") === "desc" ? -1 : 1;
  if (!key || (key === "added" && dir === 1)) return null;
  return { key, dir };
}

/** The canonical query of a sort, without "?"; "" for the default order. */
export function fileSortQuery(sort: FileSort): string {
  if (!sort || (sort.key === "added" && sort.dir === 1)) return "";
  return `sort=${sort.key}${sort.dir === -1 ? "&dir=desc" : ""}`;
}

/** A fixed filter one click away. It replaces the whole filter and is active while the filter is exactly it. */
export type QuickFilter = { id: string; label: string; filter: FileFilter };

export const QUICK_FILTERS: ReadonlyArray<QuickFilter> = [
  { id: "attention", label: N("Needs attention"), filter: { ...EMPTY_FILTER, status: ["blocker", "probe_failed", "missing"] } },
  { id: "unverified", label: N("Not verified"), filter: { ...EMPTY_FILTER, status: ["unknown"] } },
  // Missing is a state of its own, so these states never take in a missing file.
  { id: "preparable", label: N("Preparable"), filter: { ...EMPTY_FILTER, status: ["embed_open", "warning", "blocker"] } },
];

/** The quick filter the filter is exactly, if any. */
export const activeQuickFilter = (filter: FileFilter): QuickFilter | undefined =>
  QUICK_FILTERS.find((entry) => sameFilter(entry.filter, filter));

import { N } from "../../lib/i18n";
import { canonical, list, sorted, type FacetFilterSpec, type FilterEntry, type QuickFilter } from "../../lib/facetFilter";

/**
 * The files filter and its address. Status: nothing chosen shows every file, chosen states show the files in
 * any of them (OR); the server takes them as `status`. Properties overlap, so a file must have all chosen ones
 * (AND); the server takes them as `has`. Libraries (`root_id`) and extensions (`suffix`) are OR like the states,
 * `has_sidecar` is yes or no, and the search is `q`. The address uses the server's names.
 */

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
/** How the list is ordered; null is the default order, recently added with the newest first. */
export type FileSort = { key: SortKey; dir: 1 | -1 } | null;
const SORT_KEYS: ReadonlyArray<SortKey> = ["title", "status", "size", "added"];

/** The direction a key sorts in unless the address says otherwise: newest first for recently added. */
export const defaultDir = (key: SortKey): 1 | -1 => (key === "added" ? -1 : 1);

const isDefault = (sort: FileSort): boolean => !sort || (sort.key === "added" && sort.dir === -1);

/**
 * The sort an address asks for: `sort` (none or an unknown key is recently added) and `dir`, `asc` or `desc`,
 * else the key's own direction. Old links without a sort show the default order, newest first.
 */
export function parseFileSort(params: URLSearchParams): FileSort {
  const key = SORT_KEYS.find((entry) => entry === params.get("sort")) ?? "added";
  const asked = params.get("dir");
  const dir = asked === "desc" ? -1 : asked === "asc" ? 1 : defaultDir(key);
  const sort = { key, dir } as const;
  return isDefault(sort) ? null : sort;
}

/**
 * The canonical query of a sort, without "?"; "" for the default order. A direction is written only when it
 * is not the key's own, so recently added, oldest first, is `dir=asc`.
 */
export function fileSortQuery(sort: FileSort): string {
  if (!sort || isDefault(sort)) return "";
  const dir = sort.dir === defaultDir(sort.key) ? "" : `dir=${sort.dir === 1 ? "asc" : "desc"}`;
  return [sort.key === "added" ? "" : `sort=${sort.key}`, dir].filter(Boolean).join("&");
}

export const QUICK_FILTERS: ReadonlyArray<QuickFilter<FileFilter>> = [
  { id: "attention", label: N("Needs attention"), filter: { ...EMPTY_FILTER, status: ["blocker", "probe_failed", "missing"] } },
  { id: "unverified", label: N("Not verified"), filter: { ...EMPTY_FILTER, status: ["unknown"] } },
  // Missing is a state of its own, so these states never take in a missing file.
  { id: "preparable", label: N("Preparable"), filter: { ...EMPTY_FILTER, status: ["embed_open", "warning", "blocker"] } },
];

/** The quick filter the filter is exactly, if any. */
export const activeQuickFilter = (filter: FileFilter): QuickFilter<FileFilter> | undefined =>
  QUICK_FILTERS.find((entry) => sameFilter(entry.filter, filter));

/** How the files filter relates to its filter bar: the facets as the bar's value and back. */
export const FILE_FILTER_SPEC: FacetFilterSpec<FileFilter> = {
  empty: EMPTY_FILTER,
  same: sameFilter,
  isFiltered,
  quick: QUICK_FILTERS,
  searchOf: (filter) => filter.q,
  withSearch: (filter, q) => ({ ...filter, q }),
  toValue: (filter) => ({
    status: filter.status, has: filter.has, root: filter.root, suffix: filter.suffix,
    sidecar: filter.sidecar === null ? [] : [String(filter.sidecar)],
  }),
  fromValue: (value, current) => {
    const sidecar = value.sidecar?.[0];
    return {
      ...current, status: [...(value.status ?? [])], has: [...(value.has ?? [])], root: [...(value.root ?? [])], suffix: [...(value.suffix ?? [])],
      sidecar: sidecar === "true" ? true : sidecar === "false" ? false : null,
    };
  },
};

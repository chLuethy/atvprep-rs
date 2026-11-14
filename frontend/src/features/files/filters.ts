import { N } from "../../lib/i18n";

/**
 * The files filter and its address. Status: nothing chosen shows every file, chosen states show the files in
 * any of them (OR); the server takes them as `status`. Properties overlap, so a file must have all chosen ones
 * (AND); the server takes them as `has`. The search is `q`.
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
};

export const EMPTY_FILTER: FileFilter = { q: "", status: [], has: [] };

const list = (value: string | null): string[] => (value ?? "").split(",").filter(Boolean);
/** Known values only, once each, in the order of `order`. */
const canonical = (order: ReadonlyArray<string>, values: Iterable<string>): string[] => {
  const wanted = new Set(values);
  return order.filter((token) => wanted.has(token));
};

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
  return { q: (params.get("q") ?? "").trim(), status, has };
}

/** The canonical query string of a filter, without "?": `status`, then `has`, then `q`; "" for no filter. */
export function fileFilterQuery(filter: FileFilter): string {
  const parts: string[] = [];
  const status = canonical(STATE_ORDER, filter.status);
  const has = canonical(TRAIT_ORDER, filter.has);
  if (status.length > 0) parts.push(`status=${status.join(",")}`);
  if (has.length > 0) parts.push(`has=${has.join(",")}`);
  if (filter.q.trim()) parts.push(`q=${encodeURIComponent(filter.q.trim())}`);
  return parts.join("&");
}

/** Anything at all narrows the list. */
export const isFiltered = (filter: FileFilter): boolean => filter.q !== "" || filter.status.length > 0 || filter.has.length > 0;

/** Two filters show the same files. */
export const sameFilter = (a: FileFilter, b: FileFilter): boolean => fileFilterQuery(a) === fileFilterQuery(b);

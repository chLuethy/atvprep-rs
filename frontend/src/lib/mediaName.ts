/**
 * Defensive decomposition of Sonarr/Radarr file names (no extension):
 * `Series (Year) {tvdb-1} - S01E02 - Title [Tag][Tag]-GROUP`. Nothing matching
 * means the name is returned as is; the server data shape is untouched.
 */
export type MediaName =
  | { matched: false; original: string }
  | {
      matched: true;
      original: string;
      series: string;
      year: string | undefined;
      /** `S01E02`, `S01E01-E02`; absent for films. */
      code: string | undefined;
      title: string | undefined;
      ids: string[];
      tags: string[];
      group: string | undefined;
    };

const IDS = /\{([^}]+)\}/g;
const GROUP = /\]-([A-Za-z0-9_.]+)$/;
const TAGS = /((?:\s*\[[^\]]*\])+)$/;
const EPISODE = /^(.*?)\s*-\s*(S\d{1,3}E\d{1,3}(?:-?E\d{1,3})*)(?:\s*-\s*(.*))?$/i;
const SERIES = /^(.+?)\s*\((\d{4})\)$/;

export function parseMediaName(name: string): MediaName {
  const none: MediaName = { matched: false, original: name };
  let rest = name.trim();
  if (!rest) return none;

  let group: string | undefined;
  const groupMatch = rest.match(GROUP);
  if (groupMatch) {
    group = groupMatch[1] ?? "";
    rest = rest.slice(0, rest.length - group.length - 1);
  }
  let tags: string[] = [];
  const tagMatch = rest.match(TAGS);
  if (tagMatch) {
    const run = tagMatch[1] ?? "";
    tags = [...run.matchAll(/\[([^\]]*)\]/g)].map((m) => (m[1] ?? "").trim()).filter(Boolean);
    rest = rest.slice(0, rest.length - run.length);
  } else {
    group = undefined;
  }
  const ids = [...rest.matchAll(IDS)].map((m) => m[1] ?? "");
  rest = rest.replace(IDS, " ").replace(/\s+/g, " ").trim();

  const episode = rest.match(EPISODE);
  if (episode) {
    const head = (episode[1] ?? "").trim();
    const seriesYear = head.match(SERIES);
    const series = (seriesYear ? seriesYear[1] ?? head : head).trim();
    if (!series) return none;
    return { matched: true, original: name, series, year: seriesYear?.[2], code: (episode[2] ?? "").toUpperCase(), title: episode[3]?.trim() || undefined, ids, tags, group };
  }
  const film = rest.match(SERIES);
  if (film && (ids.length > 0 || tags.length > 0)) {
    return { matched: true, original: name, series: (film[1] ?? "").trim(), year: film[2], code: undefined, title: undefined, ids, tags, group };
  }
  return none;
}

/** The text a narrow list shows for a parsed name: `S01E02 Title`, or the name itself. */
export function shortName(parsed: MediaName, options: { dropSeries?: boolean; withGroup?: boolean } = {}): string {
  if (!parsed.matched) return parsed.original;
  if (!parsed.code) return parsed.year ? `${parsed.series} (${parsed.year})` : parsed.series;
  const lead = options.dropSeries ? "" : `${parsed.series}${parsed.year ? ` (${parsed.year})` : ""} `;
  const group = options.withGroup && parsed.group ? ` -${parsed.group}` : "";
  return `${lead}${parsed.code}${parsed.title ? ` ${parsed.title}` : ""}${group}`.trim();
}

/** Split for a middle ellipsis: the head shrinks with the space, the tail stays. */
export function splitMiddle(text: string, tail: number): { head: string; tail: string } {
  if (text.length <= tail * 2) return { head: text, tail: "" };
  return { head: text.slice(0, text.length - tail), tail: text.slice(text.length - tail) };
}

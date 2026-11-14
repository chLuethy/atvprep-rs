import { SessionError } from "../../lib/api";
import { t, tn } from "../../lib/i18n";
import { filterParams, type Api, type FileRow } from "./loaders";
import type { FileFilter } from "./filters";

/** How many files below a folder are in each state: clean, warning, blocker, unknown, probe_failed, missing. */
export type StateCounts = Record<string, number>;

export type BrowseFolder = {
  name: string;
  /** Set when the folder is a whole library. */
  id?: string | null;
  counts: StateCounts;
  total: number;
};

export type Browse = { folders: BrowseFolder[]; files: FileRow[]; truncated: boolean };
export type BrowseResult = ({ kind: "ok" } & Browse) | { kind: "error" } | { kind: "session" };

/**
 * The filter as the folder views take it. The search is left out (it shows a list instead), and so are the
 * libraries: `root_id` names the library being browsed, and the view leaves out the libraries not chosen.
 */
function browseFilter(filter: FileFilter | undefined, query: URLSearchParams): URLSearchParams {
  return filter ? filterParams({ ...filter, q: "", root: [] }, query) : query;
}

/** One level of the library. No root lists the libraries; a root and a path list that folder. */
export async function loadBrowse(api: Api, where: { rootId?: string | null; path?: string; filter?: FileFilter }): Promise<BrowseResult> {
  const query = new URLSearchParams();
  if (where.rootId) query.set("root_id", where.rootId);
  if (where.path) query.set("path", where.path);
  browseFilter(where.filter, query);
  try {
    const response = await api.request(`/browse${query.size > 0 ? `?${query}` : ""}`);
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: { folders?: BrowseFolder[]; files?: FileRow[]; truncated?: boolean } };
    return { kind: "ok", folders: body.data?.folders ?? [], files: body.data?.files ?? [], truncated: body.data?.truncated ?? false };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

/** The state that matters most in a folder, for the one small mark it shows: blockers, then warnings, then unverified. */
export function worstIn(counts: StateCounts): { kind: "bad" | "warn" | "neutral"; count: number; label: string } | null {
  const blockers = (counts.blocker ?? 0) + (counts.probe_failed ?? 0);
  if (blockers > 0) return { kind: "bad", count: blockers, label: tn("blocker", "blockers", blockers) };
  if ((counts.warning ?? 0) > 0) return { kind: "warn", count: counts.warning ?? 0, label: tn("warning", "warnings", counts.warning ?? 0) };
  if ((counts.missing ?? 0) > 0) return { kind: "bad", count: counts.missing ?? 0, label: t("missing") };
  if ((counts.unknown ?? 0) > 0) return { kind: "neutral", count: counts.unknown ?? 0, label: t("not verified") };
  if ((counts.convert ?? 0) > 0) return { kind: "neutral", count: counts.convert ?? 0, label: t("to convert") };
  return null;
}

/** True when a folder holds files and every one of them, at any depth, is ready. */
export function allReady(counts: StateCounts): boolean {
  const ready = counts.clean ?? 0;
  return ready > 0 && Object.values(counts).reduce((sum, number) => sum + number, 0) === ready;
}

/** The whole breakdown in words, for a tooltip and a screen reader: "2 blockers, 5 warnings, 30 ready". */
export function describeCounts(counts: StateCounts): string {
  const n = (key: string) => counts[key] ?? 0;
  const parts = [
    n("blocker") > 0 ? tn("{count} blocker", "{count} blockers", n("blocker")) : "",
    n("probe_failed") > 0 ? t("{count} inspection failed", { count: n("probe_failed") }) : "",
    n("warning") > 0 ? tn("{count} warning", "{count} warnings", n("warning")) : "",
    n("unknown") > 0 ? t("{count} not verified", { count: n("unknown") }) : "",
    n("missing") > 0 ? t("{count} missing", { count: n("missing") }) : "",
    n("convert") > 0 ? t("{count} to convert", { count: n("convert") }) : "",
    n("embed_open") > 0 ? t("{count} embedding open", { count: n("embed_open") }) : "",
    n("clean") > 0 ? t("{count} ready", { count: n("clean") }) : "",
  ].filter(Boolean);
  return parts.length > 0 ? parts.join(", ") : t("empty");
}

export type BelowResult = { kind: "ok"; files: FileRow[]; truncated: boolean } | { kind: "error" } | { kind: "session" };

/** Every file below a folder, at any depth, so an action on a folder reaches its files. */
export async function loadBelow(api: Api, where: { rootId: string; path?: string; filter?: FileFilter }): Promise<BelowResult> {
  const query = new URLSearchParams({ root_id: where.rootId });
  if (where.path) query.set("path", where.path);
  browseFilter(where.filter, query);
  try {
    const response = await api.request(`/browse/below?${query}`);
    if (!response.ok) return { kind: "error" };
    const body = (await response.json()) as { data?: { files?: FileRow[]; truncated?: boolean } };
    return { kind: "ok", files: body.data?.files ?? [], truncated: body.data?.truncated ?? false };
  } catch (error) {
    return error instanceof SessionError ? { kind: "session" } : { kind: "error" };
  }
}

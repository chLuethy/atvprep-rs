import { forget, recall, remember } from "../../lib/viewMemory";
import { fileFilterQuery, type FileFilter } from "./filters";

/** A folder path in the column view: the library, then the names of the folders opened below it. */
export type ColumnPath = { rootId: string; folders: string[] };

/**
 * What of the filter the column view follows (everything but the search), as one comparable string: the
 * columns load again when it changes, and a remembered path stays with it.
 */
export const columnFilterKey = (filter: FileFilter) => fileFilterQuery({ ...filter, q: "" });

/** Keeps the open path together with the filter it was opened under. No path (the libraries) forgets it. */
export function rememberColumnPath(filter: FileFilter, path: ColumnPath | null): void {
  if (!path) return forget("columns");
  remember("columns", JSON.stringify({ filter: columnFilterKey(filter), rootId: path.rootId, folders: path.folders }));
}

/** The remembered path when it was opened under this filter; otherwise none, and the view starts at the libraries. */
export function recallColumnPath(filter: FileFilter): ColumnPath | null {
  const saved = recall("columns");
  if (!saved) return null;
  try {
    const value = JSON.parse(saved) as { filter?: unknown; rootId?: unknown; folders?: unknown };
    if (value.filter !== columnFilterKey(filter) || typeof value.rootId !== "string" || !Array.isArray(value.folders)) return null;
    const folders = value.folders.filter((name): name is string => typeof name === "string");
    return folders.length === value.folders.length ? { rootId: value.rootId, folders } : null;
  } catch {
    return null;
  }
}

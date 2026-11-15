/**
 * The view memory: the last canonical address query of a page (filter, search and order), kept in the browser so
 * the page shows it again after a page change, a reload or in a new tab. Without storage nothing is remembered
 * and the pages behave as if they had never been visited. `columns` holds the folder path the column view of the
 * files page has open, `layout` whether that page shows the list or the columns.
 */
export type ViewPage = "files" | "jobs" | "schedules" | "columns" | "layout";

/** `layout` is the list/columns choice of the files page; it keeps the key it always had, so the choice stays. */
const slot = (page: ViewPage) => (page === "layout" ? "atvprep.library.view" : `atvprep.view.${page}`);

/** Keeps the query as the page's state. An empty query forgets it. */
export function remember(page: ViewPage, query: string): void {
  if (query === "") return forget(page);
  try { window.localStorage.setItem(slot(page), query); } catch { /* the state just is not remembered */ }
}

/** The remembered query, or null when there is none or no storage. */
export function recall(page: ViewPage): string | null {
  try { return window.localStorage.getItem(slot(page)) || null; } catch { return null; }
}

/** Drops the remembered state, so the next visit shows everything. */
export function forget(page: ViewPage): void {
  try { window.localStorage.removeItem(slot(page)); } catch { /* nothing to drop */ }
}

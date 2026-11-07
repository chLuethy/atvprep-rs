import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type Component } from "solid-js";
import { Button, Icon, Kbd } from "../../ui";
import { N, t, tn } from "../../lib/i18n";
import { autoDismiss } from "../../lib/autoDismiss";
import { ColumnsView, type ColumnAction, type Picked } from "./ColumnsView";
import { RowMenu, type MenuItem } from "./RowMenu";
import { additive, copyText, selectionMenu } from "./selection";
import { loadBelow } from "./browse";
import { hasSet, hiddenSet, knownState, normalizeHas, normalizeHidden, setGroupHidden, STATE_FILTERS, TRAIT_FILTERS, toggleHas, toggleHidden, type FilterEntry } from "./filters";
import { FileTable, type Sort, type SortKey } from "./FileTable";
import { FixPreview, type ReencodeChoice } from "./FixPreview";
import { preparable } from "./preparePlan";
import { isMp4Like } from "./convertPlan";
import { Inspector } from "./Inspector";
import { ScanControl } from "../scan/ScanControl";
import { dismissMissing, loadFiles, rawId, loadSummary, loadTraitCounts, queueJob, type Api, type FileRow, type JobResult } from "./loaders";
import { formatSize, estimateMinutes, baseName, verdictOf } from "./model";
import { isTyping } from "../../lib/keys";
import { reuseUnchanged } from "../../lib/stable";

const VIEW_KEY = "atvprep.library.view";
function readView(): "list" | "columns" {
  try { return localStorage.getItem(VIEW_KEY) === "columns" ? "columns" : "list"; } catch { return "list"; }
}

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");
const INSPECT_BATCH = 100;

/**
 * One row of tick boxes. `ticked` says which boxes are ticked: for the states that is every box that is not
 * hidden, for the properties only the chosen ones.
 */
const FilterGroup: Component<{
  title: string;
  entries: ReadonlyArray<FilterEntry>;
  ticked: (token: string) => boolean;
  count: (token: string) => number | null;
  onToggle: (token: string) => void;
  onAll?: () => void;
  onNone?: () => void;
}> = (props) => (
  <div class="filter-group" role="group" aria-label={props.title}>
    <span class="filter-title">{props.title}</span>
    <For each={props.entries}>{(entry) => {
      const shown = () => props.ticked(entry.token);
      return (
        <label class="filter-check" classList={{ off: !shown() }}>
          <input type="checkbox" checked={shown()} onChange={() => props.onToggle(entry.token)} />
          <span class="box" aria-hidden="true"><Icon name="check" size={12} /></span>
          <span class="name">{t(entry.label)}</span>
          <Show when={props.count(entry.token) !== null}>
            <span class="count" classList={{ urgent: !!entry.urgent && (props.count(entry.token) ?? 0) > 0 }}>{props.count(entry.token)}</span>
          </Show>
        </label>
      );
    }}</For>
    <Show when={props.onAll && props.onNone}>
      <span class="filter-bulk">
        <button type="button" onClick={() => props.onAll?.()}>{t("All")}</button>
        <button type="button" onClick={() => props.onNone?.()}>{t("None")}</button>
      </span>
    </Show>
  </div>
);

type Phase = "loading" | "ready" | "error" | "session";

/**
 * The library: a dense list on the left, the inspector (or a fix preview) on the right.
 * Routing stays outside: the page reports which file was opened and shows the one it is given.
 */
export const LibraryPage: Component<{
  api: Api;
  activeId?: string | undefined;
  /** One state to show only, as a link carries it; unknown values mean none. */
  status?: string | undefined;
  /** The states left out, comma separated, as the address carries them. */
  hide?: string | undefined;
  /** The properties a file must all have, comma separated, as the address carries them. */
  has?: string | undefined;
  onFilter?: (filter: { status: string; hide: string; has: string }) => void;
  onOpen: (id: string | undefined) => void;
}> = (props) => {
  const [rows, setRows] = createSignal<FileRow[]>([]);
  const [nextCursor, setNextCursor] = createSignal<string | null>(null);
  const [phase, setPhase] = createSignal<Phase>("loading");
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [moreProblem, setMoreProblem] = createSignal<string | null>(null);
  const [counts, setCounts] = createSignal<Record<string, number> | null>(null);
  const [state, setState] = createSignal(knownState(props.status));
  const [hide, setHide] = createSignal(normalizeHidden(props.hide));
  const hidden = createMemo(() => hiddenSet(hide()));
  const [has, setHas] = createSignal(normalizeHas(props.has));
  const chosen = createMemo(() => hasSet(has()));
  const [traitCounts, setTraitCounts] = createSignal<Record<string, number> | null>(null);
  const [query, setQuery] = createSignal("");
  const [search, setSearch] = createSignal("");
  const [sort, setSort] = createSignal<Sort>(null);
  const [picked, setPicked] = createSignal<ReadonlySet<string>>(new Set());
  /** The files a fix preview is open for, kept as rows because ids differ between responses. */
  const [inspectorVersion, setInspectorVersion] = createSignal(0);
  const [previewRows, setPreviewRows] = createSignal<FileRow[] | null>(null);
  const [status, setStatus] = createSignal<string | null>(null);
  autoDismiss(status, () => setStatus(null));
  const [actionError, setActionError] = createSignal<string | null>(null);

  const [jobsActive, setJobsActive] = createSignal(false);
  const [view, setViewState] = createSignal<"list" | "columns">(readView());
  // A search names files, not folders, so it always shows its hits as a list.
  const showColumns = () => view() === "columns" && query() === "";
  const none: Picked = { files: [], folders: [] };
  const [colPicked, setColPicked] = createSignal<Picked>(none);
  const [colBusy, setColBusy] = createSignal(false);
  const [clearToken, setClearToken] = createSignal(0);
  const colCount = () => colPicked().files.length + colPicked().folders.length;
  // One file has the inspector; several files, or any folder, wait for an action in the bar below.
  const columnsBar = () => showColumns() && (colCount() > 1 || colPicked().folders.length > 0);
  createEffect(() => { if (!showColumns()) setColPicked(none); });
  const setView = (next: "list" | "columns") => {
    setViewState(next);
    setPicked(new Set<string>());
    try { localStorage.setItem(VIEW_KEY, next); } catch { /* the choice just is not remembered */ }
  };
  const [missing, setMissing] = createSignal(0);
  const [convertible, setConvertible] = createSignal(0);
  const [reencodeCount, setReencodeCount] = createSignal(0);
  const [embedCount, setEmbedCount] = createSignal(0);
  const [convertedCount, setConvertedCount] = createSignal(0);
  const [removeOpen, setRemoveOpen] = createSignal(false);
  const [removeBusy, setRemoveBusy] = createSignal(false);
  const [removeError, setRemoveError] = createSignal<string | null>(null);
  let removeKey = crypto.randomUUID();
  let removeTrigger: HTMLButtonElement | undefined;

  let generation = 0;
  let pages = 1;
  // Files to mark as their rows are built: new ones and those whose state changed in the last refresh.
  let flagged = new Set<string>();
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let searchInput: HTMLInputElement | undefined;
  let previewKey: { signature: string; key: string } | undefined;
  let previewKeyConfirmed: { signature: string; key: string } | undefined;
  let verifyKey: { signature: string; key: string } | undefined;
  onCleanup(() => clearTimeout(searchTimer));

  const load = async (reset: boolean, cursor: string | null = null, quiet = false) => {
    const mine = ++generation;
    flagged = new Set();
    setMoreProblem(null);
    if (reset) {
      // Re-sorting keeps the table (and its headers) on screen until the new order arrives.
      if (!quiet) setPhase("loading");
      setPicked(new Set<string>());
      pages = 1;
    } else {
      setLoadingMore(true);
    }
    const page = await loadFiles(props.api, { q: query(), complianceState: state(), hide: hide(), has: has(), cursor, sort: sort() });
    if (mine !== generation) return;
    setLoadingMore(false);
    if (page.error) {
      // A failed "load more" must not take the rows already on screen away.
      if (!reset && rows().length > 0) {
        setMoreProblem(page.session ? t(SESSION_EXPIRED) : t("Unable to load more files. Try again."));
        return;
      }
      setPhase(page.session ? "session" : "error");
      return;
    }
    setMoreProblem(null);
    if (!reset) pages += 1;
    if (reset) setRows(page.items);
    else {
      const known = new Set(rows().map((row) => row.id));
      setRows([...rows(), ...page.items.filter((item) => !known.has(item.id))]);
    }
    setNextCursor(page.nextCursor);
    setPhase("ready");
  };

  /** Read every page loaded so far again and replace the rows in place; selection and open file stay. */
  const refreshRows = async () => {
    const mine = generation;
    let cursor: string | null = null;
    const fresh: FileRow[] = [];
    for (let index = 0; index < pages; index += 1) {
      const page = await loadFiles(props.api, { q: query(), complianceState: state(), hide: hide(), has: has(), cursor, sort: sort() });
      if (mine !== generation || page.error) return;
      fresh.push(...page.items);
      cursor = page.nextCursor;
      if (!cursor) break;
    }
    const before = new Map(rows().map((row) => [row.display_path, row]));
    const stateOf = (row: FileRow) => `${row.compliance_state}|${row.fix_status ?? ""}|${row.discovery_state}`;
    flagged = new Set(
      fresh.flatMap((row) => (row.display_path !== undefined && (!before.has(row.display_path) || stateOf(before.get(row.display_path)!) !== stateOf(row)) ? [row.display_path] : [])),
    );
    setRows(reuseUnchanged(fresh, rows(), (row) => row.display_path));
    setNextCursor(cursor);
  };
  /** What each property would show with the current selection; without an answer the whole-library counts stay. */
  const refreshTraitCounts = async () => {
    setTraitCounts(await loadTraitCounts(props.api, { state: state() || undefined, hide: hide() || undefined, has: has() || undefined }));
  };
  const refreshSummary = async () => {
    const summary = await loadSummary(props.api);
    if (!summary) return;
    if (summary.counts) setCounts(summary.counts);
    setJobsActive(summary.jobsActive);
    void refreshTraitCounts();
    setMissing(summary.missing);
    setConvertible(summary.convert);
    setReencodeCount(summary.reencode);
    setEmbedCount(summary.embed);
    setConvertedCount(summary.converted);
  };
  const openRemove = () => { removeKey = crypto.randomUUID(); setRemoveError(null); setRemoveOpen(true); };
  const closeRemove = () => { setRemoveOpen(false); queueMicrotask(() => removeTrigger?.focus()); };
  const submitRemove = async () => {
    if (removeBusy()) return;
    setRemoveBusy(true);
    setRemoveError(null);
    const result = await dismissMissing(props.api, removeKey);
    setRemoveBusy(false);
    if (!result.ok) { setRemoveError(result.session ? t(SESSION_EXPIRED) : t("Unable to remove missing files. Try again.")); return; }
    setRemoveOpen(false);
    setStatus(tn("Removed {count} missing file from the library. A file that turns up again in a scan is shown again.", "Removed {count} missing files from the library. A file that turns up again in a scan is shown again.", result.removed));
    props.onOpen(undefined);
    await load(true);
    await refreshSummary();
  };
  /** Something was just queued: follow the queue until it is empty again. */
  const follow = () => setJobsActive(true);

  // While a job waits or runs, keep the table and the open file current. The summary is read first
  // so that the last refresh after the final job still sees its result.
  createEffect(() => {
    if (!jobsActive()) return;
    const timer = window.setInterval(() => {
      if (document.hidden) return;
      void (async () => {
        await refreshSummary();
        await refreshRows();
        setInspectorVersion((value) => value + 1);
      })();
    }, 4000);
    onCleanup(() => window.clearInterval(timer));
  });

  createEffect(on(() => props.status, (next) => setState(knownState(next)), { defer: true }));
  createEffect(on(() => props.hide, (next) => setHide(normalizeHidden(next)), { defer: true }));
  createEffect(on(() => props.has, (next) => setHas(normalizeHas(next)), { defer: true }));
  createEffect(on([state, hide, has, query], () => void load(true)));
  createEffect(on([state, hide, has], () => void refreshTraitCounts()));
  createEffect(on(sort, () => void load(true, null, true), { defer: true }));
  onMount(() => void refreshSummary());

  const visibleRows = createMemo(() => rows());
  /** The picked files in list order, so job bodies are stable whatever order they were clicked in. */
  const pickedRows = createMemo(() => visibleRows().filter((row) => picked().has(row.id)));
  // A file whose preparation is already queued or running is not offered another one; the server refuses it too.
  const preparableRows = (selection: FileRow[]) => selection.filter((row) => preparable(row));
  const total = () => {
    const value = counts();
    return value ? Object.values(value).reduce((sum, n) => sum + n, 0) : null;
  };
  // Nothing found and nothing filtered out: the library has not been scanned, as opposed to a search with no hits.
  const firstRun = () => total() === 0 && !filtered() && query() === "";
  /** Anything is left out of the list besides what the search leaves out. */
  const filtered = () => state() !== "" || hide() !== "" || has() !== "";
  const changeFilter = (next: { status: string; hide: string; has: string }) => {
    setState(next.status);
    setHide(next.hide);
    setHas(next.has);
    props.onFilter?.(next);
  };
  const clearFilters = () => {
    changeFilter({ status: "", hide: "", has: "" });
    setSearch("");
    setQuery("");
  };
  const entryLabel = (token: string) => t([...STATE_FILTERS, ...TRAIT_FILTERS].find((entry) => entry.token === token)?.label ?? token);
  // The unfiltered total of each trait; the faceted count (traitCounts) wins when the server sent one.
  const traitTotal: Record<string, () => number> = { convert: convertible, reencode: reencodeCount, embed: embedCount, converted: convertedCount };
  const chipCount = (filterState: string) => {
    if (filterState === "") return total();
    if (filterState === "missing") return missing();
    if (TRAIT_FILTERS.some((entry) => entry.token === filterState)) return traitCounts()?.[filterState] ?? traitTotal[filterState]?.() ?? 0;
    return counts()?.[filterState] ?? 0;
  };

  // Where a Shift-click or Shift-arrow range starts, like the Finder's anchor.
  let anchorId: string | undefined;
  let cursorId: string | undefined;
  const togglePick = (id: string) => {
    anchorId = id;
    setPicked((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };
  /** Make these rows the selection. One row is also the open file; several close the inspector. */
  const applySelection = (ids: string[], anchor: string | undefined, cursor: string | undefined) => {
    anchorId = anchor;
    cursorId = cursor;
    setPicked(new Set(ids));
    if (ids.length === 1) props.onOpen(ids[0]); else if (props.activeId) props.onOpen(undefined);
  };
  const selectRow = (row: FileRow, event: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }) => {
    const list = visibleRows();
    if (additive(event)) {
      // Adding to a selection that is only the open file keeps that file in it.
      const base = picked().size === 0 && props.activeId && props.activeId !== row.id ? [props.activeId] : [...picked()];
      applySelection(base.includes(row.id) ? base.filter((id) => id !== row.id) : [...base, row.id], row.id, row.id);
    } else if (event.shiftKey) {
      const start = anchorId ?? props.activeId ?? row.id;
      const from = list.findIndex((item) => item.id === start);
      const to = list.findIndex((item) => item.id === row.id);
      const [low, high] = from < 0 ? [to, to] : [Math.min(from, to), Math.max(from, to)];
      applySelection(list.slice(low, high + 1).map((item) => item.id), start, row.id);
    } else {
      applySelection([row.id], row.id, row.id);
    }
  };
  const selectAllRows = () => applySelection(visibleRows().map((row) => row.id), visibleRows()[0]?.id, visibleRows().at(-1)?.id);
  const [rowMenu, setRowMenu] = createSignal<{ x: number; y: number } | null>(null);
  const onRowMenu = (row: FileRow, event: MouseEvent) => {
    event.preventDefault();
    // Like the Finder: a right click inside the selection acts on all of it, outside it on that one row.
    if (!picked().has(row.id)) applySelection([row.id], row.id, row.id);
    setRowMenu({ x: event.clientX, y: event.clientY });
  };
  const menuRows = () => (pickedRows().length > 0 ? pickedRows() : visibleRows().filter((row) => row.id === props.activeId));
  const listMenuItems = (): MenuItem[] => {
    const chosen = menuRows();
    return selectionMenu({
      many: chosen.length > 1,
      things: chosen.length > 1 ? tn("{count} file", "{count} files", chosen.length) : t("file"),
      act: (action) => void (async () => {
        if (colBusy()) return;
        setColBusy(true);
        try { await runOnRows(action, chosen, false); } finally { setColBusy(false); }
      })(),
      copyNames: () => copyText(chosen.map((row) => baseName(row.display_path ?? "").title || row.kind).join("\n")),
      copyPaths: () => copyText(chosen.map((row) => row.display_path ?? "").join("\n")),
      selectAll: selectAllRows,
    });
  };
  const onSearchInput = (value: string) => {
    setSearch(value);
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => setQuery(value.trim()), 250);
  };
  const toggleSort = (key: SortKey) => {
    const current = sort();
    setSort(current?.key === key ? (current.dir === 1 ? { key, dir: -1 } : null) : { key, dir: 1 });
  };

  /** Rows a preview or verification acts on: an explicit list, else the selection, else the open file. */
  const targets = (explicit?: string[]): FileRow[] => {
    const ids = explicit ?? (picked().size > 0 ? pickedRows().map((row) => row.id) : props.activeId ? [props.activeId] : []);
    const wanted = new Set(ids);
    return visibleRows().filter((row) => wanted.has(row.id));
  };
  const keyFor = (slot: "preview" | "verify", signature: string, confirmed = false) => {
    const current = slot === "preview" ? (confirmed ? previewKeyConfirmed : previewKey) : verifyKey;
    if (current?.signature === signature) return current.key;
    const fresh = { signature, key: crypto.randomUUID() };
    if (slot === "verify") verifyKey = fresh; else if (confirmed) previewKeyConfirmed = fresh; else previewKey = fresh;
    return fresh.key;
  };

  const openPreview = (explicit?: FileRow[]) => {
    const selection = explicit ?? targets();
    if (preparableRows(selection).length === 0) return;
    setActionError(null);
    setStatus(null);
    setPreviewRows(selection);
  };
  const previewFiles = () => previewRows() ?? [];
  const queuePrepare = async (choice: ReencodeChoice): Promise<JobResult> => {
    const confirmReencode = choice === "with";
    // Without the confirmation the files that need a re-encode stay out of the job.
    const rowsToQueue = preparableRows(previewFiles()).filter((row) => confirmReencode || !row.needs_reencode);
    const queued = new Set<string>();
    let failure: JobResult | null = null;
    const ids = rowsToQueue.map((row) => row.id);
    if (ids.length > 0) {
      const result = await queueJob(props.api, "prepare", ids, keyFor("preview", ids.join(","), confirmReencode), { confirmReencode });
      if (result.ok) ids.forEach((id) => queued.add(id));
      else failure = result;
    }
    if (queued.size > 0) {
      setRows(rows().map((row) => (queued.has(row.id) ? { ...row, fix_status: "queued" } : row)));
      setInspectorVersion((value) => value + 1);
      follow();
      setStatus(tn("Prepare job queued for {count} file. Follow it under Activity.", "Prepare job queued for {count} files. Follow it under Activity.", queued.size));
    }
    if (failure) {
      // Only the files that did not get queued stay in the preview, so a retry cannot queue the others twice.
      setPreviewRows(rowsToQueue.filter((row) => !queued.has(row.id)));
      return failure;
    }
    setPreviewRows(null);
    setPicked(new Set<string>());
    return { ok: true };
  };
  const verify = async (explicit?: string[]) => {
    // Ids go to the server as given: it accepts any id it issued, so there is no need to find
    // a matching row, and the open file's id never matches the list's id.
    const requested = explicit ?? (picked().size > 0 ? pickedRows().map((row) => row.id) : props.activeId ? [rawId(props.activeId)] : []);
    // Verify reads MP4, M4V and MOV only. A row the page knows to be another container is left out.
    const unverifiable = new Set(rows().filter((row) => !isMp4Like(row.suffix)).map((row) => row.id));
    const ids = requested.filter((id) => !unverifiable.has(id));
    const skipped = requested.length - ids.length;
    if (ids.length === 0) {
      if (skipped > 0) setActionError(t("Verify checks MP4 files only. Prepare other files first, so they become MP4s."));
      return;
    }
    setStatus(null);
    setActionError(null);
    const result = await queueJob(props.api, "verify", ids, keyFor("verify", ids.join(",")));
    if (result.ok) {
      setPicked(new Set<string>());
      setStatus(skipped > 0 ? tn("Verification job queued. {count} file was left out because only MP4 files can be verified.", "Verification job queued. {count} files were left out because only MP4 files can be verified.", skipped) : t("Verification job queued. Results appear here when it finishes."));
      follow();
    } else {
      setActionError(result.session ? t(SESSION_EXPIRED) : t("Unable to queue the verification job."));
    }
  };

  /** Re-inspect the files: an explicit list, else the selection, else the open file. */
  const inspectRows = async (explicit?: string[]) => {
    if (colBusy()) return;
    const chosen = targets(explicit);
    if (chosen.length === 0) return;
    setColBusy(true);
    try { await runOnRows("inspect", chosen, false); } finally { setColBusy(false); }
  };

  /** The files a column selection stands for: the files picked, plus every file below each folder picked. */
  const filesOfColumns = async () => {
    const seen = new Set<string>();
    const found: FileRow[] = [];
    const add = (row: FileRow) => { const key = row.display_path ?? row.id; if (!seen.has(key)) { seen.add(key); found.push(row); } };
    colPicked().files.forEach(add);
    let truncated = false;
    for (const folder of colPicked().folders) {
      if (!folder.rootId) continue;
      const result = await loadBelow(props.api, { rootId: folder.rootId, path: folder.path, state: state() || undefined, hide: hide() || undefined, has: has() || undefined });
      if (result.kind !== "ok") return { rows: found, truncated, error: result.kind };
      result.files.forEach(add);
      truncated ||= result.truncated;
    }
    return { rows: found, truncated, error: null };
  };
  /** Do `action` on these files: queue the verification, open the prepare preview, or queue inspections. */
  const runOnRows = async (action: ColumnAction, rows: FileRow[], truncated: boolean) => {
    setStatus(null);
    setActionError(null);
    const present = rows.filter((row) => row.discovery_state === "present");
    const note = truncated ? ` ${t("A folder with more than 500 files was cut off.")}` : "";
    if (action === "verify") {
      const ids = present.filter((row) => isMp4Like(row.suffix)).map((row) => row.id);
      if (ids.length === 0) { setActionError(t("Verify checks MP4 files only, and nothing selected is one.")); return; }
      const result = await queueJob(props.api, "verify", ids, crypto.randomUUID());
      if (!result.ok) { setActionError(result.session ? t(SESSION_EXPIRED) : t("Unable to queue the verification job.")); return; }
      const skipped = present.length - ids.length;
      setStatus(`${tn("Verification queued for {count} file", "Verification queued for {count} files", ids.length)}${skipped > 0 ? `; ${t("{count} skipped because they are not MP4", { count: skipped })}` : ""}.${note}`);
      setClearToken((value) => value + 1);
      setPicked(new Set<string>());
      follow();
    } else if (action === "prepare") {
      if (preparableRows(present).length === 0) { setStatus(`${t("Nothing selected needs preparing.")}${note}`); return; }
      openPreview(present);
      if (note) setStatus(note.trim());
    } else {
      // One job per 100 files, which is what the server takes in one go.
      const ids = present.map((row) => row.id);
      if (ids.length === 0) { setActionError(t("Nothing selected can be inspected: the files are missing.")); return; }
      let queued = 0;
      let session = false;
      for (let start = 0; start < ids.length; start += INSPECT_BATCH) {
        const result = await queueJob(props.api, "inspect", ids.slice(start, start + INSPECT_BATCH), crypto.randomUUID());
        if (!result.ok) { session = result.session; break; }
        queued += Math.min(INSPECT_BATCH, ids.length - start);
      }
      if (queued === 0) { setActionError(session ? t(SESSION_EXPIRED) : t("Unable to queue the inspections.")); return; }
      setStatus(`${tn("Inspection queued for {count} file", "Inspection queued for {count} files", queued)}${queued < ids.length ? `; ${t("{count} could not be queued", { count: ids.length - queued })}` : ""}.${note}`);
      setClearToken((value) => value + 1);
      setPicked(new Set<string>());
      follow();
    }
  };
  const actOnColumns = async (action: ColumnAction) => {
    if (colBusy() || colCount() === 0) return;
    setColBusy(true);
    try {
      const found = await filesOfColumns();
      if (found.error) { setStatus(null); setActionError(found.error === "session" ? t(SESSION_EXPIRED) : t("Unable to read the folders. Try again.")); return; }
      await runOnRows(action, found.rows, found.truncated);
    } finally {
      setColBusy(false);
    }
  };

  const move = (delta: number, from: string | undefined, extend = false) => {
    const list = visibleRows();
    if (list.length === 0) return;
    const index = from ? list.findIndex((row) => row.id === from) : -1;
    const next = list[Math.min(list.length - 1, Math.max(0, index + delta))];
    if (!next) return;
    if (extend) selectRow(next, { metaKey: false, ctrlKey: false, shiftKey: true });
    else applySelection([next.id], next.id, next.id);
    queueMicrotask(() => document.querySelector<HTMLElement>(`tr.row[data-id="${next.id}"]`)?.focus());
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (!showColumns() && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a" && !event.altKey && !isTyping(event.target)) {
      event.preventDefault();
      selectAllRows();
      return;
    }
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.key === "Escape") {
      if (previewRows()) setPreviewRows(null);
      else if (picked().size > 0) setPicked(new Set<string>());
      return;
    }
    if (isTyping(event.target)) return;
    const row = (event.target as HTMLElement).closest?.("tr.row") as HTMLElement | null;
    const rowId = row?.dataset.id ?? cursorId ?? props.activeId;
    switch (event.key) {
      case "ArrowDown": case "j": event.preventDefault(); move(1, rowId, event.shiftKey); break;
      case "ArrowUp": case "k": event.preventDefault(); move(-1, rowId, event.shiftKey); break;
      case "ContextMenu": if (!showColumns() && picked().size + (props.activeId ? 1 : 0) > 0) { event.preventDefault(); setRowMenu({ x: 140, y: 200 }); } break;
      case " ": case "x":
        if (!row) return;
        event.preventDefault();
        if (rowId) togglePick(rowId);
        break;
      case "f": case "F": event.preventDefault(); if (showColumns()) void actOnColumns("prepare"); else openPreview(); break;
      case "v": case "V": event.preventDefault(); if (showColumns()) void actOnColumns("verify"); else void verify(); break;
      case "i": case "I": event.preventDefault(); if (showColumns()) void actOnColumns("inspect"); else void inspectRows(); break;
      case "/": event.preventDefault(); searchInput?.focus(); break;
      default: break;
    }
  };

  const selectionCount = () => pickedRows().length;
  // A single row that is just the open file is not a "selection" worth a bar.
  const barVisible = () => selectionCount() > 0 && !(selectionCount() === 1 && props.activeId !== undefined && picked().has(props.activeId));
  const selectionPreparable = () => preparableRows(pickedRows());

  return (
    <div class="library" data-pane={props.activeId || previewRows() ? "detail" : "list"} onKeyDown={onKeyDown}>
      <section class="lib-main" aria-label={t("Library")}>
        <div class="lib-bar">
          <div class="lib-bar-top">
            <h1>{t("Library")}</h1>
            <div class="search">
              <Icon name="search" />
              <input
                ref={searchInput}
                class="input"
                type="search"
                placeholder={t("Search files")}
                aria-label={t("Search files")}
                value={search()}
                onInput={(event) => onSearchInput(event.currentTarget.value)}
              />
              <Kbd>/</Kbd>
            </div>
            <div class="seg view-switch" role="group" aria-label={t("Library view")}>
              <button type="button" aria-pressed={view() === "list"} onClick={() => setView("list")}><Icon name="rows" size={14} /> {t("List")}</button>
              <button type="button" aria-pressed={view() === "columns"} onClick={() => setView("columns")}><Icon name="columns" size={14} /> {t("Columns")}</button>
            </div>
          </div>
          <div class="lib-tools">
            <ScanControl api={props.api} onFinished={() => { void load(true); void refreshSummary(); }} />
            <Show when={missing() > 0}>
              <Button variant="quiet" class="remove-missing" ref={(element) => { removeTrigger = element; }} aria-expanded={removeOpen()} onClick={() => (removeOpen() ? closeRemove() : openRemove())}>{t("Remove missing ({count})", { count: missing() })}</Button>
            </Show>
          </div>
          <Show when={removeOpen()}>
            <div class="confirm" role="region" aria-label={t("Remove missing files")} onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); closeRemove(); } }}>
              <p>{tn("Remove {count} missing file from the library? Nothing is deleted from disk, and its job history stays.", "Remove {count} missing files from the library? Nothing is deleted from disk, and their job history stays.", missing())}</p>
              <Show when={removeError()}><p class="notice bad" role="alert">{removeError()}</p></Show>
              <div class="insp-actions">
                <Button variant="primary" disabled={removeBusy()} onClick={() => void submitRemove()}>{t("Confirm remove")}</Button>
                <Button variant="quiet" onClick={closeRemove}>{t("Keep")}</Button>
              </div>
            </div>
          </Show>
          <div class="filters" role="group" aria-label={t("Filter by status")}>
            <Show when={state() !== ""}>
              <button type="button" class="filter-only" aria-label={t("Remove filter: only {name}", { name: entryLabel(state()) })} onClick={() => changeFilter({ status: "", hide: hide(), has: has() })}>
                {t("Only {name}", { name: entryLabel(state()) })} <Icon name="x" size={12} />
              </button>
            </Show>
            <FilterGroup
              title={t("Status")}
              entries={STATE_FILTERS}
              ticked={(token) => !hidden().has(token)}
              count={chipCount}
              onToggle={(token) => changeFilter({ status: state(), hide: toggleHidden(hide(), token), has: has() })}
              onAll={() => changeFilter({ status: state(), hide: setGroupHidden(hide(), STATE_FILTERS, false), has: has() })}
              onNone={() => changeFilter({ status: state(), hide: setGroupHidden(hide(), STATE_FILTERS, true), has: has() })}
            />
            <FilterGroup
              title={t("Properties")}
              entries={TRAIT_FILTERS}
              ticked={(token) => chosen().has(token)}
              count={chipCount}
              onToggle={(token) => changeFilter({ status: state(), hide: hide(), has: toggleHas(has(), token) })}
            />
            <Show when={filtered()}>
              <Button variant="quiet" class="filter-reset" onClick={clearFilters}>{t("Show all files")}</Button>
            </Show>
          </div>
        </div>

        <Show when={status()}><p class="notice ok lib-notice" role="status">{status()}</p></Show>
        <Show when={actionError()}><p class="notice bad lib-notice" role="alert">{actionError()}</p></Show>

        <Show when={showColumns()}>
          <ColumnsView api={props.api} state={state()} hide={hide()} has={has()} version={inspectorVersion()} activeId={props.activeId} clearToken={clearToken()} onOpen={props.onOpen} onSelection={setColPicked} onAction={(action) => void actOnColumns(action)} />
        </Show>
        <div class="lib-scroll" hidden={showColumns()}>
          <Show when={phase() !== "loading"} fallback={<p class="muted pad">{t("Loading files")}</p>}>
            <Show when={phase() === "ready"} fallback={
              <div class="notice bad lib-notice" role="alert">
                <p>{phase() === "session" ? t(SESSION_EXPIRED) : t("Unable to load files")}</p>
                <Show when={phase() === "error"}><Button onClick={() => void load(true)}>{t("Retry")}</Button></Show>
              </div>
            }>
              <Show
                when={visibleRows().length > 0}
                fallback={(
                  <Show
                    when={firstRun()}
                    fallback={(
                      <div class="empty">
                        <b>{t("No files match the current filters")}</b>
                        {t("Change the status filter or clear the search.")}
                        <div><Button onClick={clearFilters}>{t("Clear filters")}</Button></div>
                      </div>
                    )}
                  >
                    <div class="empty"><b>{t("No files yet")}</b>{t("A scan reads your libraries and checks which files Apple TV will play, and why. Use Scan above to start.")}</div>
                  </Show>
                )}
              >
                <FileTable
                  rows={visibleRows()}
                  activeId={props.activeId}
                  picked={picked()}
                  sort={sort()}
                  onSort={toggleSort}
                  onSelect={selectRow}
                  onMenu={onRowMenu}
                  flagged={(row) => row.display_path !== undefined && flagged.has(row.display_path)}
                />
              </Show>
              <Show when={nextCursor()}>
                <div class="more">
                  <Show when={moreProblem()}><p class="notice bad" role="alert">{moreProblem()}</p></Show>
                  <Button disabled={loadingMore()} onClick={() => void load(false, nextCursor())}>{moreProblem() ? t("Retry loading more files") : t("Load more files")}</Button>
                </div>
              </Show>
            </Show>
          </Show>
        </div>

        <Show when={columnsBar()}>
          <div class="selbar" role="region" aria-label={t("Selection")}>
            <div class="selbar-text">
              <b>{t("{count} selected", { count: colCount() })}</b>
              <span class="muted">{colPicked().folders.length > 0 ? ` · ${tn("{count} folder, with everything below", "{count} folders, with everything below", colPicked().folders.length)}` : ""}</span>
            </div>
            <Button variant="quiet" onClick={() => setClearToken((value) => value + 1)}>{t("Clear selection")}</Button>
            <Button disabled={colBusy()} onClick={() => void actOnColumns("verify")}>{t("Verify")} <Kbd>V</Kbd></Button>
            <Button disabled={colBusy()} onClick={() => void actOnColumns("inspect")}>{t("Re-inspect")} <Kbd>I</Kbd></Button>
            <Button variant="primary" disabled={colBusy()} onClick={() => void actOnColumns("prepare")}>
              {t("Preview and prepare")} <Kbd>F</Kbd>
            </Button>
          </div>
        </Show>
        <Show when={!showColumns() && barVisible()}>
          <div class="selbar" role="region" aria-label={t("Selection")}>
            <div class="selbar-text">
              <b>{t("{count} selected", { count: selectionCount() })}</b>
              <span class="muted">
                {` · ${t("{count} can be prepared", { count: selectionPreparable().length })}`}
                {selectionPreparable().length > 0
                  ? ` · ${t("{size} to rewrite · about {minutes} min", { size: formatSize(selectionPreparable().reduce((sum, row) => sum + row.size, 0)), minutes: estimateMinutes(selectionPreparable().reduce((sum, row) => sum + row.size, 0)) })}`
                  : ""}
              </span>
            </div>
            <Button variant="quiet" onClick={() => setPicked(new Set<string>())}>{t("Clear selection")}</Button>
            <Button onClick={() => void verify(pickedRows().map((row) => row.id))}>{t("Verify selection")}</Button>
            <Button disabled={colBusy()} onClick={() => void inspectRows(pickedRows().map((row) => row.id))}>{t("Re-inspect selection")} <Kbd>I</Kbd></Button>
            <Button variant="primary" disabled={selectionPreparable().length === 0} onClick={() => openPreview()}>
              {t("Preview and prepare")} <Kbd>F</Kbd>
            </Button>
          </div>
        </Show>
      </section>

      <aside class="lib-side" aria-label={t("Details")}>
        <Button variant="quiet" class="back-to-list" onClick={() => { setPreviewRows(null); props.onOpen(undefined); }}>
          <Icon name="back" /> {t("Back to list")}
        </Button>
        <Show
          when={previewRows()}
          fallback={
            <Show when={props.activeId} fallback={
              <Show
                when={columnsBar()}
                fallback={<div class="empty"><b>{t("Select a file to inspect")}</b>{t("Click to select, Shift or Cmd to select more, right click for actions.")}</div>}
              >
                <div class="empty"><b>{tn("{count} item selected", "{count} items selected", colCount())}</b>{t("Use the bar below or right click for actions. V verifies, F previews the preparation.")}</div>
              </Show>
            }>
              {(id) => <Inspector api={props.api} fileId={id()} version={inspectorVersion()} onQueued={follow} onFix={(file) => openPreview([file])} onVerify={(fileId) => void verify([fileId])} />}
            </Show>
          }
        >
          <FixPreview
            files={preparableRows(previewFiles())}
            skipped={previewFiles().length - preparableRows(previewFiles()).length}
            onQueue={queuePrepare}
            onClose={() => setPreviewRows(null)}
          />
        </Show>
      </aside>
      <Show when={rowMenu()}>
        {(at) => <RowMenu x={at().x} y={at().y} label={t("Actions for the selection")} items={listMenuItems()} onClose={() => setRowMenu(null)} />}
      </Show>
    </div>
  );
};

export { verdictOf };

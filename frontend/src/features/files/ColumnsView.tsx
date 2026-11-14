import { batch, createEffect, createMemo, createResource, createSignal, For, on, Show, type Component } from "solid-js";
import { N, t, tn } from "../../lib/i18n";
import { Button, Icon } from "../../ui";
import { allReady, describeCounts, loadBrowse, worstIn, type Browse, type BrowseFolder, type StateCounts } from "./browse";
import type { Api, FileRow } from "./loaders";
import { EMPTY_FILTER, fileFilterQuery, type FileFilter } from "./filters";
import { verdictOf } from "./model";
import { RowMenu, type MenuItem } from "./RowMenu";
import { additive, copyText, selectionMenu, type SelectionAction } from "./selection";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");

type Entry =
  | { kind: "folder"; key: string; folder: BrowseFolder }
  | { kind: "file"; key: string; file: FileRow };
type Step = { name: string; id?: string | null };
type Place = { rootId: string | null; path: string };

/** What is selected, for the page to act on: the files themselves, and folders to reach files through. */
export type Picked = {
  files: FileRow[];
  folders: Array<{ name: string; rootId: string | null; path: string }>;
};
export type ColumnAction = SelectionAction;

/** What of the filter the column view follows (everything but the search), as one comparable string. */
const viewKey = (filter: FileFilter) => fileFilterQuery({ ...filter, q: "" });

const lastName =(path: string | undefined) => (path ?? "").split("/").pop() || t("(unnamed)");
const entriesOf = (data: Browse): Entry[] => [
  ...data.folders.map((folder): Entry => ({ kind: "folder", key: `d:${folder.name}`, folder })),
  ...data.files.map((file): Entry => ({ kind: "file", key: `f:${file.display_path ?? file.id}`, file })),
];

const Mark: Component<{ counts: StateCounts }> = (props) => {
  const worst = () => worstIn(props.counts);
  return (
    <Show
      when={worst()}
      fallback={
        <Show when={allReady(props.counts)}>
          <span class="col-state ok" title={describeCounts(props.counts)} role="img" aria-label={describeCounts(props.counts)}>
            <Icon name="check" size={12} />
          </span>
        </Show>
      }
    >
      {(mark) => (
        <span class={`col-badge ${mark().kind}`} title={describeCounts(props.counts)} aria-label={describeCounts(props.counts)}>
          <Icon name={mark().kind === "neutral" ? "dot" : mark().kind === "warn" ? "warn" : "x"} size={12} />
          {mark().count}
        </span>
      )}
    </Show>
  );
};

/** One column: what is directly inside one folder, folders first, then files. */
const Column: Component<{
  api: Api;
  index: number;
  place: Place;
  filter: FileFilter;
  version: number;
  selected: (key: string) => boolean;
  focused: boolean;
  onEntries: (index: number, entries: Entry[]) => void;
  onRow: (index: number, entry: Entry, event: MouseEvent) => void;
  onMenu: (index: number, entry: Entry, event: MouseEvent) => void;
  onBackground: (index: number) => void;
}> = (props) => {
  const [data, { refetch }] = createResource(
    () => [props.place.rootId, props.place.path, viewKey(props.filter), props.version] as const,
    ([rootId, path]) => loadBrowse(props.api, { rootId, path, filter: props.filter }),
  );
  const loaded = () => data.latest;
  // The libraries are the first column, so the library facet leaves out the ones not chosen there.
  const shown = (value: Browse): Entry[] => {
    const roots = props.filter.root;
    const entries = entriesOf(value);
    return roots.length === 0 || props.index > 0 ? entries : entries.filter((entry) => entry.kind !== "folder" || roots.includes(entry.folder.id ?? ""));
  };
  createEffect(() => {
    const value = loaded();
    if (value?.kind === "ok") props.onEntries(props.index, shown(value));
  });
  const entries = createMemo(() => {
    const value = loaded();
    return value?.kind === "ok" ? shown(value) : [];
  });

  return (
    <div
      class="col"
      classList={{ focused: props.focused }}
      data-col={props.index}
      role="listbox"
      aria-multiselectable="true"
      aria-label={props.index === 0 ? t("Libraries") : t("Folder contents")}
      onClick={(event) => { if (event.target === event.currentTarget) props.onBackground(props.index); }}
    >
      <Show when={loaded()} fallback={<p class="col-note muted">{t("Loading")}</p>}>
        {(value) => (
          <>
            <Show when={value().kind === "session"}><p class="col-note notice bad" role="alert">{t(SESSION_EXPIRED)}</p></Show>
            <Show when={value().kind === "error"}>
              <div class="col-note notice bad" role="alert"><p>{t("Unable to load this folder")}</p><Button onClick={() => void refetch()}>{t("Retry")}</Button></div>
            </Show>
            <Show when={value().kind === "ok" && entries().length === 0}>
              <p class="col-note muted">{viewKey(props.filter) ? t("No files in this state") : t("Empty folder")}</p>
            </Show>
            <For each={entries()}>{(entry) => (
              <div
                class="col-row"
                classList={{ selected: props.selected(entry.key) }}
                role="option"
                aria-selected={props.selected(entry.key)}
                data-key={entry.key}
                onClick={(event) => props.onRow(props.index, entry, event)}
                onContextMenu={(event) => props.onMenu(props.index, entry, event)}
              >
                {entry.kind === "folder" ? (
                  <>
                    <span class="col-icon folder"><Icon name="folder" size={16} /></span>
                    <span class="col-name" title={entry.folder.name}>{entry.folder.name}</span>
                    <span class="col-trail">
                      <Mark counts={entry.folder.counts} />
                      <Icon name="chevron" size={12} />
                    </span>
                  </>
                ) : (
                  <>
                    <span class="col-icon"><Icon name="file" size={16} /></span>
                    <span class="col-name" title={lastName(entry.file.display_path)}>{lastName(entry.file.display_path)}</span>
                    <span class="col-trail">
                      <span class={`col-state ${verdictOf(entry.file).kind}`} title={verdictOf(entry.file).label} role="img" aria-label={verdictOf(entry.file).label}>
                        <Icon name={verdictOf(entry.file).icon} size={12} />
                      </span>
                    </span>
                  </>
                )}
              </div>
            )}</For>
            <Show when={value().kind === "ok" && (value() as { truncated?: boolean }).truncated}>
              <p class="col-note muted">{t("Only the first files are listed. Use the search to find the rest.")}</p>
            </Show>
          </>
        )}
      </Show>
    </div>
  );
};

/**
 * The library as columns, like the Finder: libraries, then a folder's contents, then the next
 * folder's. A click selects, Cmd or Ctrl adds or removes one, Shift takes a range, Cmd+A takes the
 * column, and a right click opens a menu for what is selected. One folder selected opens its column;
 * one file opens in the inspector; several only wait for an action. Arrow keys move, right goes in,
 * left goes back.
 */
export const ColumnsView: Component<{
  api: Api;
  /** The files filter; every column follows it except for the search. Folders without a hit are left out. */
  filter?: FileFilter;
  /** Changes whenever the files may have changed under us (a job ended); columns read again. */
  version: number;
  /** The id of the file the inspector shows, if any. */
  activeId?: string | undefined;
  /** Raised to clear the selection from outside. */
  clearToken?: number;
  onOpen: (id: string | undefined) => void;
  onSelection?: (picked: Picked) => void;
  onAction?: (action: ColumnAction) => void;
}> = (props) => {
  const [trail, setTrail] = createSignal<Step[]>([]);
  const [sel, setSel] = createSignal<Entry[]>([]);
  const [anchor, setAnchor] = createSignal<string | null>(null);
  const [cursor, setCursor] = createSignal<string | null>(null);
  const [entries, setEntries] = createSignal<Record<number, Entry[]>>({});
  const [menu, setMenu] = createSignal<{ x: number; y: number } | null>(null);
  let scroller: HTMLDivElement | undefined;
  let wantFirstIn = -1;

  /** The column the selection lives in: one more than the folders opened to get there. */
  const active = () => trail().length;
  const selKeys = createMemo(() => new Set(sel().map((entry) => entry.key)));
  const selFolder = createMemo(() => {
    const only = sel().length === 1 ? sel()[0] : undefined;
    return only?.kind === "folder" ? only.folder : null;
  });
  /** Folders from the library down to the one whose contents the last column shows. */
  const chain = createMemo<Step[]>(() => {
    const folder = selFolder();
    return folder ? [...trail(), { name: folder.name, id: folder.id ?? null }] : trail();
  });
  const count = () => chain().length + 1;
  const placeOf = (index: number): Place =>
    index === 0 ? { rootId: null, path: "" } : { rootId: chain()[0]?.id ?? null, path: chain().slice(1, index).map((step) => step.name).join("/") };
  const isSelected = (index: number, key: string) =>
    index === active() ? selKeys().has(key) : index < active() ? key === `d:${trail()[index]?.name}` : false;
  const listOf = (index: number) => entries()[index] ?? [];

  const reveal = () => queueMicrotask(() => {
    scroller?.querySelector(`.col[data-col="${active()}"] .col-row.selected`)?.scrollIntoView?.({ block: "nearest" });
    const calm = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    scroller?.scrollTo?.({ left: scroller.scrollWidth, behavior: calm ? "auto" : "smooth" });
  });
  const closeInspector = () => { if (props.activeId) props.onOpen(undefined); };

  /** Make `list` the selection of column `index`, dropping the columns to its right. */
  const commit = (index: number, list: Entry[], anchorKey: string | null, cursorKey: string | null) => {
    batch(() => {
      if (index !== active()) setTrail(chain().slice(0, index));
      setEntries((all) => Object.fromEntries(Object.entries(all).filter(([key]) => Number(key) <= index)));
      setSel(list);
      setAnchor(anchorKey);
      setCursor(cursorKey);
    });
    const only = list.length === 1 ? list[0] : undefined;
    if (only?.kind === "file") props.onOpen(only.file.id); else closeInspector();
    reveal();
  };
  const selectOnly = (index: number, entry: Entry) => commit(index, [entry], entry.key, entry.key);
  const toggle = (index: number, entry: Entry) => {
    if (index !== active()) return selectOnly(index, entry);
    const next = selKeys().has(entry.key) ? sel().filter((item) => item.key !== entry.key) : [...sel(), entry];
    const order = new Map(listOf(index).map((item, position) => [item.key, position]));
    commit(index, next.sort((a, b) => (order.get(a.key) ?? 0) - (order.get(b.key) ?? 0)), entry.key, entry.key);
  };
  const range = (index: number, entry: Entry) => {
    if (index !== active()) return selectOnly(index, entry);
    const list = listOf(index);
    const from = list.findIndex((item) => item.key === (anchor() ?? cursor() ?? entry.key));
    const to = list.findIndex((item) => item.key === entry.key);
    const [low, high] = from < 0 ? [to, to] : [Math.min(from, to), Math.max(from, to)];
    commit(index, list.slice(low, high + 1), anchor() ?? entry.key, entry.key);
  };

  const onEntries = (index: number, list: Entry[]) => {
    setEntries((all) => ({ ...all, [index]: list }));
    if (wantFirstIn === index && list.length > 0 && active() === index) {
      wantFirstIn = -1;
      const first = list[0];
      if (first) selectOnly(index, first);
    }
  };

  const onRow = (index: number, entry: Entry, event: MouseEvent) => {
    if (additive(event)) toggle(index, entry);
    else if (event.shiftKey) range(index, entry);
    else selectOnly(index, entry);
  };
  const onBackground = (index: number) => { if (index !== active() || sel().length > 0) commit(index, [], null, null); };

  // A different state filter makes the folders it names a different set, so start from the libraries.
  createEffect(on(() => viewKey(props.filter ?? EMPTY_FILTER), () => {
    batch(() => { setTrail([]); setSel([]); setAnchor(null); setCursor(null); setEntries({}); });
  }, { defer: true }));
  // The inspector was closed from outside: a file that was open is no longer selected.
  createEffect(on(() => props.activeId, (id) => {
    if (!id && sel().length === 1 && sel()[0]?.kind === "file") batch(() => { setSel([]); setAnchor(null); setCursor(null); });
  }, { defer: true }));
  createEffect(on(() => props.clearToken, () => { if (sel().length > 0) commit(active(), [], null, null); }, { defer: true }));

  // Tell the page what is selected, as files and as folders it can reach files through.
  createEffect(() => {
    const list = sel();
    const level = active();
    const parent = trail().slice(1).map((step) => step.name).join("/");
    const folders = list.flatMap((entry) => entry.kind === "folder"
      ? [{ name: entry.folder.name, rootId: level === 0 ? entry.folder.id ?? null : trail()[0]?.id ?? null, path: level === 0 ? "" : parent ? `${parent}/${entry.folder.name}` : entry.folder.name }]
      : []);
    props.onSelection?.({ files: list.flatMap((entry) => (entry.kind === "file" ? [entry.file] : [])), folders });
  });

  const move = (delta: number, extend: boolean) => {
    const list = listOf(active());
    if (list.length === 0) return;
    const at = cursor() ? list.findIndex((entry) => entry.key === cursor()) : -1;
    const next = at < 0 ? (delta > 0 ? 0 : list.length - 1) : Math.max(0, Math.min(list.length - 1, at + delta));
    const entry = list[next];
    if (!entry) return;
    if (extend) range(active(), entry); else selectOnly(active(), entry);
  };
  const into = () => {
    const folder = selFolder();
    if (!folder) return;
    const next = active() + 1;
    batch(() => { setTrail(chain()); setSel([]); setAnchor(null); setCursor(null); });
    const first = listOf(next)[0];
    if (first) selectOnly(next, first); else wantFirstIn = next;
  };
  const out = () => {
    if (active() === 0) return;
    const step = trail()[active() - 1];
    const entry = listOf(active() - 1).find((item) => item.key === `d:${step?.name}`);
    batch(() => { setTrail(trail().slice(0, -1)); setSel(entry ? [entry] : []); setAnchor(entry?.key ?? null); setCursor(entry?.key ?? null); });
    closeInspector();
    reveal();
  };
  const selectAll = () => {
    const list = listOf(active());
    commit(active(), list, list[0]?.key ?? null, list[list.length - 1]?.key ?? null);
  };

  const openMenuAtCursor = () => {
    if (sel().length === 0) return;
    const row = scroller?.querySelector(`.col[data-col="${active()}"] .col-row.selected`);
    const box = row?.getBoundingClientRect();
    setMenu(box ? { x: box.left + 24, y: box.bottom } : { x: 80, y: 80 });
  };
  const onMenu = (index: number, entry: Entry, event: MouseEvent) => {
    event.preventDefault();
    // Like the Finder: a right click inside the selection acts on all of it, outside it on that one row.
    if (index !== active() || !selKeys().has(entry.key)) selectOnly(index, entry);
    setMenu({ x: event.clientX, y: event.clientY });
  };

  const onKeyDown = (event: KeyboardEvent) => {
    const command = event.metaKey || event.ctrlKey;
    if (event.altKey) return;
    const handled = (() => {
      if (command && event.key.toLowerCase() === "a") { selectAll(); return true; }
      if (command) return false;
      switch (event.key) {
        case "ArrowDown": case "j": move(1, event.shiftKey); return true;
        case "ArrowUp": case "k": move(-1, event.shiftKey); return true;
        case "Home": move(-Infinity, event.shiftKey); return true;
        case "End": move(Infinity, event.shiftKey); return true;
        case "ArrowRight": case "Enter": into(); return true;
        case "ArrowLeft": out(); return true;
        case "Escape": if (sel().length > 0) { commit(active(), [], null, null); return true; } return false;
        case "ContextMenu": openMenuAtCursor(); return true;
        case "F10": if (event.shiftKey) { openMenuAtCursor(); return true; } return false;
        default: return false;
      }
    })();
    if (handled) { event.preventDefault(); event.stopPropagation(); }
  };

  /** A click on the path bar: back to that folder, selected in its own column. */
  const goTo = (length: number) => {
    if (length === 0) return commit(0, [], null, null);
    const column = length - 1;
    const step = chain()[column];
    const entry = listOf(column).find((item) => item.key === `d:${step?.name}`);
    commit(column, entry ? [entry] : [], entry?.key ?? null, entry?.key ?? null);
  };

  const run = (action: ColumnAction) => { props.onAction?.(action); queueMicrotask(() => scroller?.focus()); };
  const pathOf = (entry: Entry) => {
    if (entry.kind === "file") return entry.file.display_path ?? lastName(entry.file.display_path);
    const parent = trail().slice(1).map((step) => step.name).join("/");
    return active() === 0 ? entry.folder.name : parent ? `${parent}/${entry.folder.name}` : entry.folder.name;
  };
  const menuItems = (): MenuItem[] => {
    const list = sel();
    return selectionMenu({
      many: list.length > 1,
      things: list.length > 1 ? tn("{count} item", "{count} items", list.length) : list[0]?.kind === "folder" ? t("folder") : t("file"),
      act: run,
      copyNames: () => copyText(list.map((entry) => (entry.kind === "folder" ? entry.folder.name : lastName(entry.file.display_path))).join("\n")),
      copyPaths: () => copyText(list.map(pathOf).join("\n")),
      selectAll: () => { selectAll(); scroller?.focus(); },
    });
  };

  const crumbs = () => [{ label: t("Libraries"), length: 0 }, ...chain().map((step, index) => ({ label: step.name, length: index + 1 }))];
  const summary = createMemo(() => {
    // The rightmost column is what is on screen to open next, so it is the one the status line counts.
    const list = listOf(chain().length);
    const total: StateCounts = {};
    for (const entry of list) {
      if (entry.kind === "folder") for (const [key, number] of Object.entries(entry.folder.counts)) total[key] = (total[key] ?? 0) + number;
      else { const key = entry.file.discovery_state === "missing" ? "missing" : entry.file.compliance_state; total[key] = (total[key] ?? 0) + 1; }
    }
    const folders = list.filter((entry) => entry.kind === "folder").length;
    const files = list.length - folders;
    const things = [folders > 0 ? tn("{count} folder", "{count} folders", folders) : "", files > 0 ? tn("{count} file", "{count} files", files) : ""].filter(Boolean).join(", ");
    const picked = sel().length > 1 ? `${t("{count} selected", { count: sel().length })} · ` : "";
    return list.length === 0 && picked === "" ? "" : `${picked}${list.length === 0 ? "" : `${things} · ${describeCounts(total)}`}`;
  });

  return (
    <div class="cols-wrap">
      <div ref={scroller} class="cols" tabindex="0" role="group" aria-label={t("Files by folder. Arrow keys move, Shift extends the selection, right opens a folder, left goes back.")} onKeyDown={onKeyDown}>
        <For each={Array.from({ length: count() }, (_, index) => index)}>{(index) => (
          <Column
            api={props.api}
            index={index}
            place={placeOf(index)}
            filter={props.filter ?? EMPTY_FILTER}
            version={props.version}
            selected={(key) => isSelected(index, key)}
            focused={active() === index}
            onEntries={onEntries}
            onRow={onRow}
            onMenu={onMenu}
            onBackground={onBackground}
          />
        )}</For>
      </div>
      <nav class="pathbar" aria-label={t("Location")}>
        <For each={crumbs()}>{(crumb, position) => (
          <>
            <Show when={position() > 0}><Icon name="chevron" size={10} /></Show>
            <button type="button" class="crumb" aria-current={position() === crumbs().length - 1 ? "location" : undefined} onClick={() => goTo(crumb.length)}>{crumb.label}</button>
          </>
        )}</For>
      </nav>
      <p class="statusbar muted" role="status">{summary()}</p>
      <Show when={menu()}>
        {(at) => <RowMenu x={at().x} y={at().y} label={t("Actions for the selection")} items={menuItems()} onClose={() => setMenu(null)} />}
      </Show>
    </div>
  );
};

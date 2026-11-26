import { createEffect, createMemo, For, onSettled, Show, type Component } from "solid-js";
import { createRowWindow } from "../../lib/rowWindow";
import { uniqueRowKeys } from "../../lib/rowKeys";
import { fileKey } from "./identity";
import { flash } from "../../lib/flash";
import { StatusChip } from "../../ui";
import type { FileRow } from "./loaders";
import type { FileSort } from "./filters";
import { N, t } from "../../lib/i18n";
import { baseName, formatSize, verdictOf } from "./model";
import { changeLabel, needLabel } from "./preparePlan";

/** The keys a column header sorts by; "recently added" has no column. */
export type SortKey = "title" | "status" | "size";
export type Sort = FileSort;

/** Library, Encoding need and Change drop out of a narrow list (`opt`): the name and the status stay. */
const SORTABLE: Record<SortKey, { label: string; className: string }> = {
  title: { label: N("File"), className: "" },
  status: { label: N("Status"), className: "g" },
  size: { label: N("Size"), className: "g num" },
};

/** The dense list. It only renders and reports; selection and loading live in the page. */
export const FileTable: Component<{
  rows: FileRow[];
  activeId: string | undefined;
  picked: ReadonlySet<string>;
  sort: Sort;
  onSort: (key: SortKey) => void;
  /** A click on a row: plain, with Cmd or Ctrl, or with Shift. The page decides what each means. */
  onSelect: (file: FileRow, event: MouseEvent) => void;
  onMenu: (file: FileRow, event: MouseEvent) => void;
  /** Rows to mark as they are built: files that turned up or changed state while the list was open. */
  flagged?: (file: FileRow) => boolean;
  /** The name of a library by its id; an unknown id shows as it is. */
  libraryName?: (rootId: string) => string;
  /** Asks for a row to be built and focused, e.g. when J or K walks past the rows that are rendered. A new object each time. */
  reveal?: { id: string };
}> = (props) => {
  const rowKey = createMemo(() => uniqueRowKeys(props.rows, fileKey));
  let table: HTMLTableElement | undefined;
  let body: HTMLTableSectionElement | undefined;
  // Only the rows near the visible part exist in the page; a long list does not grow the DOM with every page loaded.
  const window = createRowWindow({
    total: () => props.rows.length,
    scroller: () => table?.closest<HTMLElement>(".lib-scroll"),
    origin: () => table?.tHead?.offsetHeight ?? 0,
    rendered: () => Array.from(body?.querySelectorAll("tr.row") ?? []),
  });
  onSettled(() => window.attach());
  createEffect(() => props.rows.length, () => window.recompute());
  const slice = createMemo(() => {
    const { start, end } = window.shown();
    return props.rows.slice(start, end);
  });
  const above = () => window.shown().start * window.rowHeight();
  const below = () => (props.rows.length - window.shown().end) * window.rowHeight();
  createEffect(() => props.reveal, (ask) => {
    if (!ask) return;
    const index = props.rows.findIndex((row) => row.id === ask.id);
    if (index < 0) return;
    window.reveal(index);
    onSettled(() => {
      const element = body?.querySelector<HTMLElement>(`tr.row[data-id="${CSS.escape(ask.id)}"]`);
      element?.focus();
      element?.scrollIntoView?.({ block: "nearest" });
    });
  });
  // The list has one stop for the keyboard: the open file, else the first row that is built.
  const tabId = () => (slice().some((row) => row.id === props.activeId) ? props.activeId : slice()[0]?.id);
  const tabbable = (id: string) => tabId() === id;
  const header = (key: SortKey) => {
    const column = SORTABLE[key];
    const direction = () => (props.sort?.key === key ? (props.sort.dir === 1 ? "ascending" : "descending") : undefined);
    return (
      <th scope="col" class={column.className} aria-sort={direction() ?? "none"}>
        <button type="button" class="th-btn" onClick={() => props.onSort(key)}>
          {t(column.label)}
          <span aria-hidden="true">{direction() === "ascending" ? " ↑" : direction() === "descending" ? " ↓" : ""}</span>
        </button>
      </th>
    );
  };

  return (
    <table class="files-table" aria-label={t("Media files")} aria-rowcount={props.rows.length + 1} ref={table}>
      <thead>
        <tr>
          {header("title")}
          <th scope="col" class="g opt">{t("Library")}</th>
          {header("status")}
          <th scope="col" class="g opt">{t("Encoding need")}</th>
          <th scope="col" class="g opt">{t("Change")}</th>
          {header("size")}
        </tr>
      </thead>
      <tbody ref={body}>
        <Show when={above() > 0}><tr aria-hidden="true" class="row-gap"><td colspan="6" style={{ height: `${above()}px`, padding: 0 }} /></tr></Show>
        <For each={slice()} keyed={(file) => rowKey()(file)}>{(file, index) => {
          const name = () => baseName(file().display_path ?? "");
          const title = () => name().title || file().kind;
          const verdict = () => verdictOf(file());
          let row: HTMLTableRowElement | undefined;
          createEffect(() => ({ file: file(), flagged: props.flagged?.(file()) }), ({ flagged }) => { if (row && flagged) flash(row); });
          return (
            <tr
              ref={row}
              class="row"
              data-id={file().id}
              tabindex={tabbable(file().id) ? 0 : -1}
              aria-rowindex={window.shown().start + index() + 2}
              aria-selected={props.activeId === file().id || props.picked.has(file().id) ? "true" : "false"}
              data-picked={props.picked.has(file().id) ? "true" : undefined}
              onClick={(event) => {
                if ((event.target as HTMLElement).closest("input, a")) return;
                props.onSelect(file(), event);
              }}
              onContextMenu={(event) => props.onMenu(file(), event)}
            >
              <td class="file-cell">
                <a
                  class="file-title"
                  tabindex="-1"
                  href={`/files/${file().id}`}
                  onClick={(event) => { event.preventDefault(); props.onSelect(file(), event); }}
                >{title()}</a>
                <Show when={name().folder}><span class="file-folder mono">{name().folder}</span></Show>
                <Show when={file().last_interrupted && !file().fix_status}><span class="interrupted-note">{t("Last preparation interrupted")}</span></Show>
              </td>
              <td class="g opt muted">{props.libraryName?.(file().root_id) ?? file().root_id}</td>
              <td class="g col-status"><StatusChip kind={verdict().kind} icon={verdict().icon}>{verdict().label}</StatusChip></td>
              <td class="g opt">{needLabel(file().encode_need)}</td>
              <td class="g opt">{changeLabel(file().planned_change)}</td>
              <td class="g num">{formatSize(file().size)}</td>
            </tr>
          );
        }}</For>
        <Show when={below() > 0}><tr aria-hidden="true" class="row-gap"><td colspan="6" style={{ height: `${below()}px`, padding: 0 }} /></tr></Show>
      </tbody>
    </table>
  );
};

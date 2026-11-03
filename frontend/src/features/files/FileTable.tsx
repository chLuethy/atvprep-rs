import { For, onMount, Show, type Component } from "solid-js";
import { flash } from "../../lib/flash";
import { StatusChip } from "../../ui";
import type { FileRow } from "./loaders";
import { N, t } from "../../lib/i18n";
import { baseName, formatSize, parseReason, verdictOf } from "./model";

export type SortKey = "title" | "status" | "size";
export type Sort = { key: SortKey; dir: 1 | -1 } | null;

const COLUMNS: ReadonlyArray<{ key: SortKey; label: string; className?: string }> = [
  { key: "title", label: N("File") },
  { key: "status", label: N("Status") },
  { key: "size", label: N("Size"), className: "num" },
];

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
}> = (props) => {
  const tabbable = (id: string, index: number) => (props.activeId ? props.activeId === id : index === 0);
  const header = (column: (typeof COLUMNS)[number]) => {
    const direction = () => (props.sort?.key === column.key ? (props.sort.dir === 1 ? "ascending" : "descending") : undefined);
    return (
      <th scope="col" class={[column.className, column.key === "status" ? "col-status" : ""].filter(Boolean).join(" ")} aria-sort={direction() ?? "none"}>
        <button type="button" class="th-btn" onClick={() => props.onSort(column.key)}>
          {t(column.label)}
          <span aria-hidden="true">{direction() === "ascending" ? " ↑" : direction() === "descending" ? " ↓" : ""}</span>
        </button>
      </th>
    );
  };

  return (
    <table class="files-table" aria-label={t("Media files")}>
      <thead>
        <tr>
          <For each={COLUMNS.slice(0, 2)}>{header}</For>
          <th scope="col" class="col-reason">{t("First reason")}</th>
          {header(COLUMNS[2]!)}
        </tr>
      </thead>
      <tbody>
        <For each={props.rows}>{(file, index) => {
          const name = () => baseName(file.display_path ?? "");
          const title = () => name().title || file.kind;
          const verdict = () => verdictOf(file);
          const reason = () => {
            // A file that is gone has nothing to say about its streams; the old verify reason would mislead.
            if (file.discovery_state === "missing") return t("No longer on disk");
            const first = (file.compliance_reasons ?? [])[0];
            return first && file.compliance_state !== "clean" ? parseReason(first).message : "";
          };
          let row: HTMLTableRowElement | undefined;
          onMount(() => { if (row && props.flagged?.(file)) flash(row); });
          return (
            <tr
              ref={row}
              class="row"
              data-id={file.id}
              tabindex={tabbable(file.id, index()) ? 0 : -1}
              aria-selected={props.activeId === file.id || props.picked.has(file.id)}
              data-picked={props.picked.has(file.id) ? "true" : undefined}
              onClick={(event) => {
                if ((event.target as HTMLElement).closest("input, a")) return;
                props.onSelect(file, event);
              }}
              onContextMenu={(event) => props.onMenu(file, event)}
            >
              <td class="file-cell">
                <a
                  class="file-title"
                  href={`/files/${file.id}`}
                  onClick={(event) => { event.preventDefault(); props.onSelect(file, event); }}
                >{title()}</a>
                <Show when={name().folder}><span class="file-folder mono">{name().folder}</span></Show>
              </td>
              <td class="col-status"><StatusChip kind={verdict().kind} icon={verdict().icon}>{verdict().label}</StatusChip></td>
              <td class="col-reason muted"><span class="reason-text">{reason()}</span></td>
              <td class="num">{formatSize(file.size)}</td>
            </tr>
          );
        }}</For>
      </tbody>
    </table>
  );
};

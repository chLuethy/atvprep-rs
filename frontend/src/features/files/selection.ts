import { t } from "../../lib/i18n";
import type { MenuItem } from "./RowMenu";

export const MAC = /Mac|iPhone|iPad/.test(typeof navigator === "undefined" ? "" : navigator.platform);

/** Cmd on a Mac, Ctrl elsewhere: add or remove one row from the selection. (Ctrl-click on a Mac is a right click.) */
export const additive = (event: { metaKey: boolean; ctrlKey: boolean }) => event.metaKey || (event.ctrlKey && !MAC);

export type SelectionAction = "verify" | "prepare" | "inspect";

/** The context menu for a selection, the same wherever the selection was made. */
export function selectionMenu(options: {
  /** "file", "folder" or "3 items", for the labels. */
  things: string;
  many: boolean;
  act: (action: SelectionAction) => void;
  copyNames: () => void;
  copyPaths: () => void;
  selectAll: () => void;
}): MenuItem[] {
  return [
    { id: "verify", label: t("Verify {things}", { things: options.things }), hint: "V", run: () => options.act("verify") },
    { id: "prepare", label: t("Prepare preview…"), hint: "F", run: () => options.act("prepare") },
    { id: "inspect", label: t("Re-inspect {things}", { things: options.things }), hint: "I", run: () => options.act("inspect") },
    { id: "copy-name", label: options.many ? t("Copy names") : t("Copy name"), separated: true, run: options.copyNames },
    { id: "copy-path", label: options.many ? t("Copy paths") : t("Copy path"), run: options.copyPaths },
    { id: "all", label: t("Select all"), hint: MAC ? "⌘A" : "Ctrl+A", separated: true, run: options.selectAll },
  ];
}

/** Put text on the clipboard; a blocked clipboard is not worth a message. */
export function copyText(text: string): void {
  try { void navigator.clipboard?.writeText(text); } catch { /* nothing to do */ }
}

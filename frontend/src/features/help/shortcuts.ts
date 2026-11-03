/**
 * Every key the interface answers to, grouped by where it works. A chord is keys pressed together;
 * the chords of one item are alternatives. Keep this in step with the handlers: the library and
 * activity pages, the shell and the command palette.
 */
import { N } from "../../lib/i18n";

export type Shortcut = { chords: string[][]; does: string };
export type ShortcutGroup = { title: string; items: Shortcut[] };

export const SHORTCUTS: ReadonlyArray<ShortcutGroup> = [
  {
    title: N("Everywhere"),
    items: [
      { chords: [["?"]], does: N("Show these shortcuts") },
      { chords: [["⌘", "K"], ["Ctrl", "K"]], does: N("Open the commands: go to a page, filter files, change the theme") },
      { chords: [["Esc"]], does: N("Close the open panel, preview or palette") },
    ],
  },
  {
    title: N("Library"),
    items: [
      { chords: [["↓"], ["J"]], does: N("Next file") },
      { chords: [["↑"], ["K"]], does: N("Previous file") },
      { chords: [["Shift", "↓"], ["Shift", "↑"]], does: N("Extend the selection") },
      { chords: [["⌘", "A"], ["Ctrl", "A"]], does: N("Select every file shown") },
      { chords: [["Space"], ["X"]], does: N("Add the file to the selection or take it out") },
      { chords: [["→"]], does: N("Columns view: open the folder") },
      { chords: [["←"]], does: N("Columns view: back to the folder before") },
      { chords: [["F"]], does: N("Preview a fix for the selected files") },
      { chords: [["V"]], does: N("Verify the selected files") },
      { chords: [["I"]], does: N("Re-inspect the selected files") },
      { chords: [["Shift", "F10"]], does: N("Open the menu for the selection") },
      { chords: [["/"]], does: N("Search files") },
      { chords: [["Esc"]], does: N("Let go of the selection or close the preview") },
    ],
  },
  {
    title: N("Schedules"),
    items: [
      { chords: [["N"]], does: N("New schedule") },
      { chords: [["Esc"]], does: N("Close the editor") },
    ],
  },
  {
    title: N("Activity"),
    items: [
      { chords: [["↓"], ["J"]], does: N("Next job") },
      { chords: [["↑"], ["K"]], does: N("Previous job") },
    ],
  },
];

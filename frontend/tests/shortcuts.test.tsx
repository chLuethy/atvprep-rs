import { fireEvent, render, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import App from "../src/App";
import { ShortcutsPanel } from "../src/features/help/ShortcutsPanel";
import { SHORTCUTS } from "../src/features/help/shortcuts";

afterEach(() => {
  window.localStorage.clear();
  window.history.replaceState(null, "", "/");
});

describe("the shortcut list", () => {
  it("groups what works everywhere apart from what works on one page, and names every key", () => {
    expect(SHORTCUTS.map((group) => group.title)).toEqual(["Everywhere", "Files", "Schedules", "Activity"]);
    for (const group of SHORTCUTS) {
      expect(group.items.length).toBeGreaterThan(0);
      for (const item of group.items) {
        expect(item.does.length).toBeGreaterThan(3);
        expect(item.chords.length).toBeGreaterThan(0);
        for (const chord of item.chords) expect(chord.every((key) => key.length > 0)).toBe(true);
      }
    }
    const everywhere = SHORTCUTS[0]!.items.map((item) => item.does);
    expect(everywhere).toEqual(expect.arrayContaining([expect.stringMatching(/commands/i), expect.stringMatching(/shortcuts/i)]));
  });

  it("describes the keys that are really bound on the files page", () => {
    const library = SHORTCUTS[1]!.items.flatMap((item) => item.chords.flat());
    for (const key of ["J", "K", "Space", "F", "V", "/", "Esc"]) expect(library).toContain(key);
  });

  it("renders as a labelled list of keys and what they do", () => {
    const view = render(() => <ShortcutsPanel />);

    const library = view.getByRole("group", { name: "Files" });
    expect(within(library).getByText("Add the file to the selection or take it out")).toBeInTheDocument();
    expect(library.querySelectorAll("kbd").length).toBeGreaterThan(4);
  });
});

describe("shortcuts in the shell", () => {
  const press = (key: string, target: Document | HTMLElement = document) => fireEvent.keyDown(target, { key });

  it("opens a non-blocking reference with ? and closes it with Escape", async () => {
    const view = render(() => <App />);
    expect(view.queryByRole("region", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();

    await press("?");
    const panel = view.getByRole("region", { name: "Keyboard shortcuts" });
    expect(panel).toBeInTheDocument();
    expect(view.queryByRole("dialog", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();
    expect(view.getByRole("main")).toBeInTheDocument();

    await press("Escape", panel);
    expect(view.queryByRole("region", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();
  });

  it("toggles with ? and leaves ? alone while someone is typing", async () => {
    const view = render(() => <App />);
    await press("?");
    await press("?");
    expect(view.queryByRole("region", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();

    await fireEvent.keyDown(document, { key: "k", metaKey: true });
    const search = view.getByRole("combobox", { name: "Search commands" });
    await fireEvent.keyDown(search, { key: "?" });
    expect(view.queryByRole("region", { name: "Keyboard shortcuts" })).not.toBeInTheDocument();
  });

  it("is reachable from the sidebar and from the command palette", async () => {
    const view = render(() => <App />);

    await fireEvent.click(view.getByRole("button", { name: /keyboard shortcuts/i }));
    expect(view.getByRole("region", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    await press("Escape", view.getByRole("region", { name: "Keyboard shortcuts" }));

    await fireEvent.keyDown(document, { key: "k", metaKey: true });
    const search = view.getByRole("combobox", { name: "Search commands" });
    await fireEvent.input(search, { target: { value: "shortcuts" } });
    await fireEvent.keyDown(search, { key: "Enter" });
    expect(view.getByRole("region", { name: "Keyboard shortcuts" })).toBeInTheDocument();
  });
});

describe("the one-time hint", () => {
  it("tells a first-time visitor where the keyboard help is, once", async () => {
    const first = render(() => <App />);
    const hint = first.getByRole("note", { name: "Keyboard tip" });
    expect(hint).toHaveTextContent(/press \? for the keyboard shortcuts/i);

    await fireEvent.click(within(hint).getByRole("button", { name: "Got it" }));
    expect(first.queryByRole("note", { name: "Keyboard tip" })).not.toBeInTheDocument();
    first.unmount();

    const second = render(() => <App />);
    expect(second.queryByRole("note", { name: "Keyboard tip" })).not.toBeInTheDocument();
  });

  it("goes away by itself once the shortcut help has been opened", async () => {
    const view = render(() => <App />);
    expect(view.getByRole("note", { name: "Keyboard tip" })).toBeInTheDocument();

    await fireEvent.keyDown(document, { key: "?" });

    expect(view.queryByRole("note", { name: "Keyboard tip" })).not.toBeInTheDocument();
  });
});

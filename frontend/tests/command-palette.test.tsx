import { fireEvent, render, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { CommandPalette, type Command } from "../src/ui/CommandPalette";

const make = () => {
  const run = { library: vi.fn(), dark: vi.fn(), lock: vi.fn() };
  const commands: Command[] = [
    { id: "library", label: "Go to Library", run: run.library },
    { id: "dark", label: "Theme: Dark", hint: "Appearance", run: run.dark },
    { id: "lock", label: "Lock manager", run: run.lock },
  ];
  return { run, commands };
};

describe("CommandPalette", () => {
  it("renders nothing while closed", () => {
    const { commands } = make();
    const view = render(() => <CommandPalette open={false} commands={commands} onClose={vi.fn()} />);

    expect(view.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("opens with the search focused and every command listed", async () => {
    const { commands } = make();
    const view = render(() => <CommandPalette open commands={commands} onClose={vi.fn()} />);

    expect(view.getByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
    await waitFor(() => expect(document.activeElement).toBe(view.getByRole("combobox", { name: "Search commands" })));
    expect(view.getAllByRole("option").map((option) => option.textContent)).toEqual([
      expect.stringContaining("Go to Library"),
      expect.stringContaining("Theme: Dark"),
      expect.stringContaining("Lock manager"),
    ]);
  });

  it("filters as you type, ignoring case, and says when nothing matches", async () => {
    const { commands } = make();
    const view = render(() => <CommandPalette open commands={commands} onClose={vi.fn()} />);
    const search = view.getByRole("combobox", { name: "Search commands" });

    await fireEvent.input(search, { target: { value: "DARK" } });
    expect(view.getAllByRole("option")).toHaveLength(1);
    expect(view.getByRole("option")).toHaveTextContent("Theme: Dark");

    await fireEvent.input(search, { target: { value: "zzz" } });
    expect(view.queryAllByRole("option")).toHaveLength(0);
    expect(view.getByText("No command found")).toBeInTheDocument();
  });

  it("puts matches at the start of a word before matches in the middle of one", async () => {
    const commands: Command[] = [
      { id: "blocker", label: "Library: show Blocker", run: vi.fn() },
      { id: "block", label: "Unblock a file", run: vi.fn() },
      { id: "lock", label: "Lock manager", run: vi.fn() },
      { id: "dark", label: "Appearance: Dark", run: vi.fn() },
    ];
    const view = render(() => <CommandPalette open commands={commands} onClose={vi.fn()} />);

    await fireEvent.input(view.getByRole("combobox", { name: "Search commands" }), { target: { value: "lock" } });

    expect(view.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "Lock manager",
      "Library: show Blocker",
      "Unblock a file",
    ]);
  });

  it("keeps the given order among equally good matches", async () => {
    const commands: Command[] = [
      { id: "a", label: "Go to Library", run: vi.fn() },
      { id: "b", label: "Go to Activity", run: vi.fn() },
    ];
    const view = render(() => <CommandPalette open commands={commands} onClose={vi.fn()} />);

    await fireEvent.input(view.getByRole("combobox", { name: "Search commands" }), { target: { value: "go" } });

    expect(view.getAllByRole("option").map((option) => option.textContent)).toEqual(["Go to Library", "Go to Activity"]);
  });

  it("runs the highlighted command on Enter and closes", async () => {
    const { commands, run } = make();
    const onClose = vi.fn();
    const view = render(() => <CommandPalette open commands={commands} onClose={onClose} />);
    const search = view.getByRole("combobox", { name: "Search commands" });

    expect(view.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    await fireEvent.keyDown(search, { key: "ArrowDown" });
    expect(view.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
    await fireEvent.keyDown(search, { key: "Enter" });

    expect(run.dark).toHaveBeenCalledOnce();
    expect(run.library).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("keeps the highlight inside the list and resets it when the query changes", async () => {
    const { commands } = make();
    const view = render(() => <CommandPalette open commands={commands} onClose={vi.fn()} />);
    const search = view.getByRole("combobox", { name: "Search commands" });

    await fireEvent.keyDown(search, { key: "ArrowUp" });
    expect(view.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    await fireEvent.keyDown(search, { key: "ArrowDown" });
    await fireEvent.keyDown(search, { key: "ArrowDown" });
    await fireEvent.keyDown(search, { key: "ArrowDown" });
    expect(view.getAllByRole("option")[2]).toHaveAttribute("aria-selected", "true");
    await fireEvent.input(search, { target: { value: "o" } });
    expect(view.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
  });

  it("closes on Escape without running anything", async () => {
    const { commands, run } = make();
    const onClose = vi.fn();
    const view = render(() => <CommandPalette open commands={commands} onClose={onClose} />);

    await fireEvent.keyDown(view.getByRole("combobox", { name: "Search commands" }), { key: "Escape" });

    expect(onClose).toHaveBeenCalled();
    expect(Object.values(run).every((fn) => fn.mock.calls.length === 0)).toBe(true);
  });

  it("runs a command that is clicked", async () => {
    const { commands, run } = make();
    const onClose = vi.fn();
    const view = render(() => <CommandPalette open commands={commands} onClose={onClose} />);

    await fireEvent.click(view.getByRole("option", { name: /Lock manager/ }));

    expect(run.lock).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalled();
  });

  it("hands focus back to whatever had it before the palette opened", async () => {
    const { commands } = make();
    const trigger = document.createElement("button");
    document.body.append(trigger);
    trigger.focus();
    const [isOpen, setIsOpen] = createSignal(true);
    const view = render(() => <CommandPalette open={isOpen()} commands={commands} onClose={() => setIsOpen(false)} />);
    await waitFor(() => expect(document.activeElement).toBe(view.getByRole("combobox", { name: "Search commands" })));

    await fireEvent.keyDown(view.getByRole("combobox", { name: "Search commands" }), { key: "Escape" });

    await waitFor(() => expect(document.activeElement).toBe(trigger));
    trigger.remove();
  });
});

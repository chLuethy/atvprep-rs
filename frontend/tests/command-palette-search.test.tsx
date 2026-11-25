import { fireEvent, render } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommandPalette, type Command } from "../src/ui/CommandPalette";

const hit = (id: string, label: string, run = vi.fn()): Command => ({ id, label, hint: "File", run });
const commands = (): Command[] => [
  { id: "files", label: "Go to Files", run: vi.fn() },
  { id: "lock", label: "Lock manager", run: vi.fn() },
  { id: "dark", label: "Theme: Dark", run: vi.fn() },
];

describe("CommandPalette search hits", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("asks the search once, after a pause, and shows its hits with the match highlighted", async () => {
    const search = vi.fn(async (_query: string) => [hit("file-1", "Maisel S01E02 Titel")]);
    const view = render(() => <CommandPalette open commands={commands()} search={search} onClose={vi.fn()} />);
    const box = view.getByRole("combobox", { name: "Search commands" });

    await fireEvent.input(box, { target: { value: "m" } });
    await fireEvent.input(box, { target: { value: "mai" } });
    expect(search).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(400);
    await vi.advanceTimersByTimeAsync(0);

    expect(search).toHaveBeenCalledOnce();
    expect(search).toHaveBeenCalledWith("mai");
    expect(view.getByRole("option", { name: /Maisel/ }).querySelector("mark")?.textContent).toBe("Mai");
  });

  it("does not ask for a blank query and lists the commands", async () => {
    const search = vi.fn(async () => [hit("file-1", "Maisel")]);
    const view = render(() => <CommandPalette open commands={commands()} search={search} onClose={vi.fn()} />);
    await fireEvent.input(view.getByRole("combobox", { name: "Search commands" }), { target: { value: "  " } });
    await vi.advanceTimersByTimeAsync(400);

    expect(search).not.toHaveBeenCalled();
    expect(view.getAllByRole("option")).toHaveLength(3);
  });

  it("drops an answer that arrives after the query changed", async () => {
    let first: (value: Command[]) => void = () => undefined;
    const search = vi.fn((query: string) => query === "old"
      ? new Promise<Command[]>((resolve) => { first = resolve; })
      : Promise.resolve([hit("file-2", "Newer")]));
    const view = render(() => <CommandPalette open commands={commands()} search={search} onClose={vi.fn()} />);
    const box = view.getByRole("combobox", { name: "Search commands" });

    await fireEvent.input(box, { target: { value: "old" } });
    await vi.advanceTimersByTimeAsync(400);
    await fireEvent.input(box, { target: { value: "newer" } });
    await vi.advanceTimersByTimeAsync(400);
    first([hit("file-1", "Stale old hit")]);
    await vi.advanceTimersByTimeAsync(0);

    expect(view.queryByText(/Stale/)).not.toBeInTheDocument();
    expect(view.getByRole("option", { name: /Newer/ })).toBeInTheDocument();
  });

  it("runs a hit on Enter and closes", async () => {
    const run = vi.fn();
    const onClose = vi.fn();
    const view = render(() => <CommandPalette open commands={[]} search={async () => [hit("file-1", "Maisel", run)]} onClose={onClose} />);
    const box = view.getByRole("combobox", { name: "Search commands" });
    await fireEvent.input(box, { target: { value: "mai" } });
    await vi.advanceTimersByTimeAsync(400);
    await fireEvent.keyDown(box, { key: "Enter" });

    expect(run).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalled();
  });

  it("a failing search leaves the commands working", async () => {
    const view = render(() => <CommandPalette open commands={commands()} search={async () => { throw new Error("boom"); }} onClose={vi.fn()} />);
    await fireEvent.input(view.getByRole("combobox", { name: "Search commands" }), { target: { value: "lock" } });
    await vi.advanceTimersByTimeAsync(400);

    expect(view.getByRole("option", { name: /Lock manager/ })).toBeInTheDocument();
  });
});

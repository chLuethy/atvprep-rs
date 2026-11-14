import { fireEvent, render, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { FilterBar, type Facet, type FacetCounts, type FilterValue, type QuickChoice } from "../src/ui/FilterBar";

const FACETS: Facet[] = [
  { key: "status", title: "Status", logic: "or", options: [
    { value: "clean", label: "Ready" }, { value: "warning", label: "Warning" }, { value: "blocker", label: "Blocker" }, { value: "missing", label: "Missing" },
  ] },
  { key: "has", title: "Properties", logic: "and", options: [{ value: "embed", label: "Subtitles embeddable" }, { value: "convert", label: "To convert" }] },
];
const COUNTS: FacetCounts = { status: { clean: 3, warning: 2, blocker: 1, missing: 0 }, has: { embed: 2, convert: 0 } };

/** The bar as a page uses it: it reports a change, the page feeds the new value back in. */
function show(initial: FilterValue = {}, over: { counts?: FacetCounts | null; facets?: Facet[]; onReset?: () => void; dirty?: boolean; quick?: QuickChoice[]; onQuick?: (id: string) => void } = {}) {
  const [value, setValue] = createSignal<FilterValue>(initial);
  const onChange = vi.fn((next: FilterValue) => setValue(next));
  const view = render(() => (
    <FilterBar
      label="Filter files"
      facets={over.facets ?? FACETS}
      value={value()}
      counts={over.counts === undefined ? COUNTS : over.counts}
      summary="4 of 6 files"
      dirty={over.dirty}
      onChange={onChange}
      onReset={over.onReset}
      quick={over.quick}
      onQuick={over.onQuick}
    />
  ));
  return { view, onChange, value, setValue };
}

const openFacet = async (view: ReturnType<typeof show>["view"], name: RegExp) => {
  await fireEvent.click(view.getByRole("button", { name }));
  return view.getByRole("dialog", { name: name.source.replace(/^\^/, "").split(":")[0] });
};

describe("FilterBar: the head", () => {
  it("says how many items are left as a polite live region", () => {
    const { view } = show();
    const summary = view.getByText("4 of 6 files");
    expect(summary).toHaveAttribute("aria-live", "polite");
  });

  it("offers Reset filters only while something is filtered, and clears every facet with it", async () => {
    const { view, onChange } = show();
    expect(view.queryByRole("button", { name: "Reset filters" })).not.toBeInTheDocument();
    view.unmount();

    const active = show({ status: ["warning"], has: ["embed"] });
    await fireEvent.click(active.view.getByRole("button", { name: "Reset filters" }));
    expect(active.onChange).toHaveBeenLastCalledWith({});
    expect(onChange).not.toHaveBeenCalled();
  });

  it("leaves resetting to the page when it filters more than the facets, like a search", async () => {
    const onReset = vi.fn();
    const { view, onChange } = show({}, { dirty: true, onReset });
    await fireEvent.click(view.getByRole("button", { name: "Reset filters" }));
    expect(onReset).toHaveBeenCalledTimes(1);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("FilterBar: a facet button", () => {
  it("shows only the title while nothing is chosen", () => {
    const { view } = show();
    const button = view.getByRole("button", { name: "Status" });
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(button).toHaveAttribute("aria-haspopup", "dialog");
    expect(view.queryByRole("button", { name: /^Clear/ })).not.toBeInTheDocument();
  });

  it("is filled and names up to two chosen values, from three on only how many", () => {
    const { view } = show({ status: ["warning", "blocker"] });
    expect(view.getByRole("button", { name: "Status: Warning, Blocker" })).toHaveAttribute("data-active", "true");
    view.unmount();

    const three = show({ status: ["clean", "warning", "blocker"] });
    expect(three.view.getByRole("button", { name: "Status: 3" })).toBeInTheDocument();
  });

  it("clears only its own facet with its ✕", async () => {
    const { view, onChange } = show({ status: ["warning"], has: ["embed"] });
    await fireEvent.click(view.getByRole("button", { name: "Clear Status" }));
    expect(onChange).toHaveBeenLastCalledWith({ status: [], has: ["embed"] });
  });

  it("is locked when no option has a hit and nothing is chosen", () => {
    const { view } = show({}, { counts: { status: COUNTS.status!, has: { embed: 0, convert: 0 } } });
    expect(view.getByRole("button", { name: "Properties" })).toBeDisabled();
    expect(view.getByRole("button", { name: "Status" })).toBeEnabled();
  });

  it("is never locked while the counts are unknown", () => {
    const { view } = show({}, { counts: null });
    expect(view.getByRole("button", { name: "Properties" })).toBeEnabled();
  });
});

describe("FilterBar: the popover", () => {
  it("lists the options with their counts and applies a choice at once, staying open", async () => {
    const { view, onChange } = show();
    const dialog = await openFacet(view, /^Status/);
    expect(view.getByRole("button", { name: "Status" })).toHaveAttribute("aria-expanded", "true");
    expect(within(dialog).getByText("Any of these")).toBeInTheDocument();

    await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Warning 2" }));
    expect(onChange).toHaveBeenLastCalledWith({ status: ["warning"] });
    await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Blocker 1" }));
    expect(onChange).toHaveBeenLastCalledWith({ status: ["warning", "blocker"] });
    expect(within(dialog).getByRole("checkbox", { name: "Warning 2" })).toBeChecked();
    expect(view.getByRole("dialog", { name: "Status" })).toBeInTheDocument();

    await fireEvent.click(within(dialog).getByRole("checkbox", { name: "Warning 2" }));
    expect(onChange).toHaveBeenLastCalledWith({ status: ["blocker"] });
  });

  it("says that a file needs all chosen properties", async () => {
    const { view } = show();
    const dialog = await openFacet(view, /^Properties/);
    expect(within(dialog).getByText("All of these")).toBeInTheDocument();
  });

  it("dims an option without hits, but it can still be chosen", async () => {
    const { view, onChange } = show();
    const dialog = await openFacet(view, /^Status/);
    const missing = within(dialog).getByRole("checkbox", { name: "Missing 0" });
    expect(missing).toBeEnabled();
    expect(missing.closest("label")).toHaveAttribute("data-empty", "true");
    expect(within(dialog).getByRole("checkbox", { name: "Ready 3" }).closest("label")).not.toHaveAttribute("data-empty");
    await fireEvent.click(missing);
    expect(onChange).toHaveBeenLastCalledWith({ status: ["missing"] });
  });

  it("resets its own facet and nothing else", async () => {
    const { view, onChange } = show({ status: ["warning"], has: ["embed"] });
    const dialog = await openFacet(view, /^Status/);
    await fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }));
    expect(onChange).toHaveBeenLastCalledWith({ status: [], has: ["embed"] });
  });

  it("closes with Close, with Escape and when another facet opens, giving the focus back to its button", async () => {
    const { view } = show();
    const dialog = await openFacet(view, /^Status/);
    await fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(view.queryByRole("dialog")).not.toBeInTheDocument();
    expect(view.getByRole("button", { name: "Status" })).toHaveFocus();

    const again = await openFacet(view, /^Status/);
    await fireEvent.keyDown(within(again).getByRole("checkbox", { name: "Ready 3" }), { key: "Escape" });
    expect(view.queryByRole("dialog")).not.toBeInTheDocument();
    expect(view.getByRole("button", { name: "Status" })).toHaveFocus();

    await openFacet(view, /^Status/);
    await fireEvent.click(view.getByRole("button", { name: "Properties" }));
    expect(view.getAllByRole("dialog")).toHaveLength(1);
    expect(view.getByRole("dialog", { name: "Properties" })).toBeInTheDocument();
  });

  it("moves between the options with the arrow keys, starting on the first", async () => {
    const { view } = show();
    const dialog = await openFacet(view, /^Status/);
    const ready = within(dialog).getByRole("checkbox", { name: "Ready 3" });
    expect(ready).toHaveFocus();
    await fireEvent.keyDown(ready, { key: "ArrowDown" });
    expect(within(dialog).getByRole("checkbox", { name: "Warning 2" })).toHaveFocus();
    await fireEvent.keyDown(within(dialog).getByRole("checkbox", { name: "Warning 2" }), { key: "ArrowUp" });
    expect(ready).toHaveFocus();
    await fireEvent.keyDown(ready, { key: "ArrowUp" });
    expect(within(dialog).getByRole("checkbox", { name: "Missing 0" })).toHaveFocus();
  });

  it("offers a single choice as radio buttons that replace each other", async () => {
    const facets: Facet[] = [{ key: "order", title: "Order", logic: "or", multiple: false, options: [{ value: "asc", label: "Oldest first" }, { value: "desc", label: "Newest first" }] }];
    const { view, onChange } = show({}, { facets, counts: null });
    const dialog = await openFacet(view, /^Order/);
    await fireEvent.click(within(dialog).getByRole("radio", { name: "Oldest first" }));
    expect(onChange).toHaveBeenLastCalledWith({ order: ["asc"] });
    await fireEvent.click(within(dialog).getByRole("radio", { name: "Newest first" }));
    expect(onChange).toHaveBeenLastCalledWith({ order: ["desc"] });
  });
});

describe("FilterBar: all filters at once", () => {
  it("has a filter button that shows every facet in one panel", async () => {
    const { view, onChange } = show({ has: ["embed"] });
    await fireEvent.click(view.getByRole("button", { name: "All filters (1 active)" }));
    const panel = view.getByRole("dialog", { name: "All filters" });
    expect(within(panel).getByRole("group", { name: "Status" })).toBeInTheDocument();
    expect(within(panel).getByRole("checkbox", { name: "Subtitles embeddable 2" })).toBeChecked();
    await fireEvent.click(within(panel).getByRole("checkbox", { name: "Blocker 1" }));
    expect(onChange).toHaveBeenLastCalledWith({ has: ["embed"], status: ["blocker"] });
    await fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
    expect(view.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

const EXTENSIONS = [".avi", ".m4v", ".mkv", ".mov", ".mp4"].map((value) => ({ value, label: value.slice(1) }));
const WITH_MORE: Facet[] = [
  ...FACETS,
  { key: "suffix", title: "Extension", logic: "or", placement: "more", searchable: true, options: EXTENSIONS },
  { key: "sidecar", title: "Subtitle files", logic: "or", multiple: false, placement: "more", options: [{ value: "true", label: "Yes" }, { value: "false", label: "No" }] },
];

describe("FilterBar: a page that builds its facets anew", () => {
  it("keeps the popover and the focused option when the facets come back as new objects", async () => {
    const [value, setValue] = createSignal<FilterValue>({});
    // Like a page whose options depend on the counts: every change hands in fresh facet and option objects.
    const facets = () => { void value(); return FACETS.map((facet) => ({ ...facet, options: facet.options.map((option) => ({ ...option })) })); };
    const view = render(() => <FilterBar label="Filter files" facets={facets()} value={value()} counts={COUNTS} onChange={setValue} />);
    await fireEvent.click(view.getByRole("button", { name: "Status" }));
    const warning = within(view.getByRole("dialog", { name: "Status" })).getByRole("checkbox", { name: "Warning 2" });
    warning.focus();
    await fireEvent.click(warning);

    expect(warning).toBeChecked();
    expect(warning.isConnected).toBe(true);
    expect(warning).toHaveFocus();
  });
});

describe("FilterBar: more filters", () => {
  it("keeps the rarer facets in a second row that opens in place", async () => {
    const { view } = show({}, { facets: WITH_MORE, counts: null });
    const more = view.getByRole("button", { name: "More filters" });
    expect(more).toHaveAttribute("aria-expanded", "false");
    expect(view.queryByRole("button", { name: "Extension" })).not.toBeInTheDocument();
    expect(view.getByRole("button", { name: "Status" })).toBeInTheDocument();

    await fireEvent.click(more);
    expect(more).toHaveAttribute("aria-expanded", "true");
    const row = view.getByRole("group", { name: "More filters" });
    expect(more).toHaveAttribute("aria-controls", row.id);
    expect(within(row).getByRole("button", { name: "Extension" })).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: "Subtitle files" })).toBeInTheDocument();
    await fireEvent.click(more);
    expect(view.queryByRole("group", { name: "More filters" })).not.toBeInTheDocument();
  });

  it("stays open while a facet in it is active, and closes once that is cleared", async () => {
    const { view } = show({ sidecar: ["true"] }, { facets: WITH_MORE, counts: null });
    const more = view.getByRole("button", { name: "More filters" });
    expect(more).toHaveAttribute("aria-expanded", "true");
    expect(more).toBeDisabled();
    const row = view.getByRole("group", { name: "More filters" });
    await fireEvent.click(within(row).getByRole("button", { name: "Clear Subtitle files" }));
    expect(view.queryByRole("group", { name: "More filters" })).not.toBeInTheDocument();
    expect(view.getByRole("button", { name: "More filters" })).toBeEnabled();
  });

  it("does not offer the row when every facet is in the main row", () => {
    const { view } = show();
    expect(view.queryByRole("button", { name: "More filters" })).not.toBeInTheDocument();
  });
});

describe("FilterBar: a searchable popover", () => {
  it("narrows the options as one types and starts in the search field", async () => {
    const { view, onChange } = show({}, { facets: WITH_MORE, counts: null });
    await fireEvent.click(view.getByRole("button", { name: "More filters" }));
    const dialog = await openFacet(view, /^Extension/);
    const find = within(dialog).getByRole("searchbox", { name: "Search Extension" });
    await Promise.resolve();
    expect(find).toHaveFocus();
    expect(within(dialog).getAllByRole("checkbox")).toHaveLength(5);

    await fireEvent.input(find, { target: { value: "M4" } });
    expect(within(dialog).getAllByRole("checkbox").map((box) => box.closest("label")?.textContent)).toEqual(["m4v"]);
    await fireEvent.keyDown(find, { key: "ArrowDown" });
    expect(within(dialog).getByRole("checkbox", { name: "m4v" })).toHaveFocus();
    await fireEvent.click(within(dialog).getByRole("checkbox", { name: "m4v" }));
    expect(onChange).toHaveBeenLastCalledWith({ suffix: [".m4v"] });

    await fireEvent.input(find, { target: { value: "webm" } });
    expect(within(dialog).queryAllByRole("checkbox")).toHaveLength(0);
    expect(within(dialog).getByText("Nothing matches")).toBeInTheDocument();
  });

  it("has no search field unless the facet asks for one", async () => {
    const { view } = show();
    const dialog = await openFacet(view, /^Status/);
    expect(within(dialog).queryByRole("searchbox")).not.toBeInTheDocument();
  });
});

describe("FilterBar: quick filters", () => {
  it("offers each as a toggle that shows whether it is in effect", async () => {
    const onQuick = vi.fn();
    const { view } = show({}, { quick: [{ id: "attention", label: "Needs attention", active: true }, { id: "unverified", label: "Not verified", active: false }], onQuick });
    const group = view.getByRole("group", { name: "Quick filters" });
    expect(within(group).getByRole("button", { name: "Needs attention" })).toHaveAttribute("aria-pressed", "true");
    const unverified = within(group).getByRole("button", { name: "Not verified" });
    expect(unverified).toHaveAttribute("aria-pressed", "false");
    await fireEvent.click(unverified);
    expect(onQuick).toHaveBeenCalledWith("unverified");
  });

  it("shows no group without quick filters", () => {
    const { view } = show();
    expect(view.queryByRole("group", { name: "Quick filters" })).not.toBeInTheDocument();
  });
});

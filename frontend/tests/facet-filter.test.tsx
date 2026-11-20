import { createRoot, createSignal, flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createFacetFilter, type FacetFilterSpec } from "../src/lib/facetFilter";

// The mechanics every filter bar shares: what the address says, the search with its pause, the quick
// filters, the mapping to the bar's value and the reset. A page only hands over its spec.
type Colors = { q: string; color: string[]; order: "asc" | "desc" };
const EMPTY: Colors = { q: "", color: [], order: "asc" };
const key = (f: Colors) => JSON.stringify(f);

const spec: FacetFilterSpec<Colors> = {
  empty: EMPTY,
  same: (a, b) => key(a) === key(b),
  isFiltered: (f) => f.q !== "" || f.color.length > 0,
  keep: (f) => ({ order: f.order }),
  quick: [{ id: "warm", label: "Warm", filter: { ...EMPTY, color: ["red"] } }],
  searchOf: (f) => f.q,
  withSearch: (f, q) => ({ ...f, q }),
  toValue: (f) => ({ color: f.color }),
  fromValue: (value, current) => ({ ...current, color: [...(value.color ?? [])] }),
};

const setup = (initial?: Colors) => {
  const calls: Array<{ filter: Colors; replace: boolean }> = [];
  let set!: (next: Colors | undefined) => void;
  const facet = createRoot(() => {
    const [address, setAddress] = createSignal<Colors | undefined>(initial);
    set = setAddress;
    return createFacetFilter(spec, { filter: address, onFilter: (filter, how) => calls.push({ filter, replace: how.replace }) });
  });
  return { facet, calls, set };
};

describe("createFacetFilter", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("starts from the address's filter and shows its search", () => {
    const { facet } = setup({ ...EMPTY, q: "dune", color: ["blue"] });
    expect(facet.filter().color).toEqual(["blue"]);
    expect(facet.search()).toBe("dune");
    expect(facet.bar().dirty).toBe(true);
    expect(facet.bar().value).toEqual({ color: ["blue"] });
  });

  it("hands a chosen facet to the address as a new history entry", () => {
    const { facet, calls } = setup();
    facet.bar().onChange({ color: ["green"] });
    flush();
    expect(facet.filter().color).toEqual(["green"]);
    expect(calls).toEqual([{ filter: { ...EMPTY, color: ["green"] }, replace: false }]);
  });

  it("takes a search only after a short pause, and replaces the history entry", () => {
    const { facet, calls } = setup();
    facet.onSearchInput("d");
    facet.onSearchInput("du");
    flush();
    expect(facet.search()).toBe("du");
    vi.advanceTimersByTime(249);
    expect(calls).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(calls).toEqual([{ filter: { ...EMPTY, q: "du" }, replace: true }]);
  });

  it("resets everything but what the spec keeps, and drops a pending search", () => {
    const { facet, calls } = setup({ ...EMPTY, color: ["red"], order: "desc" });
    facet.onSearchInput("late");
    facet.bar().onReset?.();
    flush();
    vi.advanceTimersByTime(500);
    expect(facet.search()).toBe("");
    expect(facet.filter()).toEqual({ ...EMPTY, order: "desc" });
    expect(calls).toEqual([{ filter: { ...EMPTY, order: "desc" }, replace: false }]);
  });

  it("applies a quick filter, and takes it away when it is chosen again", () => {
    const { facet } = setup({ ...EMPTY, order: "desc" });
    facet.bar().onQuick?.("warm");
    flush();
    expect(facet.filter()).toEqual({ ...EMPTY, color: ["red"], order: "desc" });
    expect(facet.bar().quick).toEqual([{ id: "warm", label: "Warm", active: true }]);
    facet.bar().onQuick?.("warm");
    flush();
    expect(facet.filter()).toEqual({ ...EMPTY, order: "desc" });
    expect(facet.bar().quick?.[0]?.active).toBe(false);
  });

  it("follows the address when it changes from outside, search field included", () => {
    const { facet, calls, set } = setup();
    set({ ...EMPTY, q: "back", color: ["blue"] });
    flush();
    expect(facet.filter().color).toEqual(["blue"]);
    expect(facet.search()).toBe("back");
    expect(calls).toEqual([]);
  });

  it("does not tell the address about a filter that shows the same", () => {
    const { facet, calls } = setup({ ...EMPTY, color: ["red"] });
    facet.bar().onChange({ color: ["red"] });
    expect(calls).toEqual([]);
  });
});

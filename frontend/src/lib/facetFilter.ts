/**
 * The mechanics every list with a filter bar shares, once. A list hands over its facets as a spec and gets:
 *
 * - `useFilterAddress`: the filter in the address and the view memory, for the route of the page;
 * - `createFacetFilter`: the filter as the page works with it (the search with its pause, quick filters,
 *   the mapping to the filter bar's value, the reset), for the page itself.
 *
 * Also here: the small helpers of an address (comma lists, canonical order) and the shape of a facet option
 * and a quick filter.
 */
import { useLocation, useNavigate } from "@solidjs/router";
import { createEffect, createMemo, createSignal, onCleanup, untrack, type Accessor } from "solid-js";
import type { FilterValue, QuickChoice } from "../ui/FilterBar";
import { t } from "./i18n";
import { recall, remember, type ViewPage } from "./viewMemory";

/** One option of a facet as a page lists it: its token in the address and its label. */
export type FilterEntry = { token: string; label: string; urgent?: boolean };

/** A fixed filter one click away. It replaces the whole filter and is active while the filter is exactly it. */
export type QuickFilter<F> = { id: string; label: string; filter: F };

/** The non-empty entries of a comma list in the address. */
export const list = (value: string | null): string[] => (value ?? "").split(",").filter(Boolean);

/** Known values only, once each, in the order of `order`. */
export const canonical = (order: ReadonlyArray<string>, values: Iterable<string>): string[] => {
  const wanted = new Set(values);
  return order.filter((token) => wanted.has(token));
};

/** Values nobody lists in advance (ids, extensions): once each, sorted. */
export const sorted = (values: Iterable<string>): string[] => [...new Set(values)].sort();

/** How long the search waits for the next keystroke before it becomes part of the filter. */
export const SEARCH_PAUSE_MS = 250;

/**
 * Marks an address the page itself chose, so a bare one there means "show everything" (Reset filters, a cleared
 * quick filter, Back to such an entry). A bare address without the mark came from outside (navigation, the palette,
 * a link, a reload) and takes the last remembered state instead, by replacing the address so no history entry is
 * added. A query of its own always wins.
 */
export const OWN = { own: true } as const;
const isOwn = (state: unknown) => typeof state === "object" && state !== null && (state as { own?: unknown }).own === true;

/** How the address of a page maps to what the page shows (`S`: the filter and, say, the order). */
export type FilterAddressSpec<S> = {
  /** The view memory slot of the page. */
  page: ViewPage;
  /** The path of the page now, without the query (an open item is part of it). */
  path: () => string;
  parse: (params: URLSearchParams) => S;
  /** The canonical query, without "?"; "" for nothing chosen. */
  query: (state: S) => string;
  same: (a: S, b: S) => boolean;
};

/**
 * Keeps the address of a page and its view memory in step: a bare address from outside takes the remembered
 * state, every other one is rewritten to its canonical query and remembered, an empty one forgetting the state.
 * `state` is what the address asks for, `go` writes a new state to the address: a history entry, or with
 * `replace` (a search, so typing adds none) a replacement of the current one.
 */
export function useFilterAddress<S>(spec: FilterAddressSpec<S>) {
  const navigate = useNavigate();
  const location = useLocation();
  const at = (query: string) => `${spec.path()}${query ? `?${query}` : ""}`;
  const state = createMemo(() => spec.parse(new URLSearchParams(location.search)), { equals: spec.same });
  createEffect(() => ({ search: location.search.replace(/^\?/, ""), own: isOwn(location.state), query: spec.query(state()), path: spec.path() }),
    ({ search, own, query, path }) => {
      const url = (value: string) => `${path}${value ? `?${value}` : ""}`;
      if (search === "" && !own) {
        navigate(url(recall(spec.page) ?? ""), { replace: true, state: OWN });
        return;
      }
      remember(spec.page, query);
      if (search !== query) navigate(url(query), { replace: true, state: OWN });
    });
  const go = (next: S, how: { replace: boolean } = { replace: false }) =>
    navigate(at(spec.query(next)), { replace: how.replace, state: OWN });
  /** Another path of the same page (an item opened), keeping the query. */
  const open = (path: string) => navigate(`${path}${location.search}`, { state: OWN });
  return { state, go, open };
}

/** What a filter bar needs of the filter, besides the facets and counts a page knows itself. */
export type FilterBarProps = {
  value: FilterValue;
  dirty: boolean;
  onChange: (next: FilterValue) => void;
  onReset: () => void;
  quick: ReadonlyArray<QuickChoice>;
  onQuick: (id: string) => void;
};

/** How a list's filter relates to its filter bar. */
export type FacetFilterSpec<F> = {
  /** The filter that shows everything. */
  empty: F;
  /** Two filters show the same. */
  same: (a: F, b: F) => boolean;
  /** Anything at all narrows the list. */
  isFiltered: (filter: F) => boolean;
  /** What a reset and a quick filter keep of the current filter, such as the order. */
  keep?: (current: F) => Partial<F>;
  quick?: ReadonlyArray<QuickFilter<F>>;
  /** The search of a filter and a filter with another one; both or neither. */
  searchOf?: (filter: F) => string;
  withSearch?: (filter: F, search: string) => F;
  /** The filter as the bar shows it, and the bar's value as a filter, the rest of `current` kept. */
  toValue: (filter: F) => FilterValue;
  fromValue: (value: FilterValue, current: F) => F;
};

/**
 * The filter of a list as its page works with it. It starts from `input.filter` (what the address asks for),
 * follows it when it changes from outside (Back, a link), and tells `input.onFilter` about every filter the page
 * chooses. `replace` is set for a search, so typing does not add a history entry per pause.
 */
export function createFacetFilter<F extends object>(
  spec: FacetFilterSpec<F>,
  input: { filter: Accessor<F | undefined>; onFilter?: ((filter: F, how: { replace: boolean }) => void) | undefined },
) {
  const [filter, setFilter] = createSignal<F>(() => input.filter() ?? spec.empty, { equals: spec.same });
  const searchOf = (value: F) => spec.searchOf?.(value) ?? "";
  /** What the search field shows; the filter takes it after a short pause. */
  const [search, setSearch] = createSignal(untrack(() => searchOf(filter())));
  /** The filter the page last chose or the address last asked for; an echo of it is no outside change. */
  let shown = untrack(filter);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));

  /** Chooses a filter: the page follows it at once and the address after it. */
  const change = (next: F, replace = false) => {
    if (spec.same(next, filter())) return;
    shown = next;
    setFilter(() => next);
    input.onFilter?.(next, { replace });
  };
  /** Chooses a filter whose search the field has to show too. */
  const replaceAll = (next: F) => {
    clearTimeout(timer);
    setSearch(searchOf(next));
    change(next);
  };
  const kept = (): Partial<F> => spec.keep?.(filter()) ?? {};

  // Only a filter that differs from what the page shows is a change from outside: it takes the search field
  // and drops a pending search. The echo of the page's own choice leaves both alone.
  createEffect(input.filter, (next) => {
    const wanted = next ?? spec.empty;
    if (spec.same(wanted, shown)) return;
    shown = wanted;
    clearTimeout(timer);
    setSearch(searchOf(wanted));
  }, { defer: true });

  const onSearchInput = (value: string) => {
    setSearch(value);
    clearTimeout(timer);
    timer = setTimeout(() => {
      const withSearch = spec.withSearch;
      if (withSearch) change(withSearch(filter(), value.trim()), true);
    }, SEARCH_PAUSE_MS);
  };
  const reset = () => replaceAll({ ...spec.empty, ...kept() });
  const activeQuick = () => spec.quick?.find((entry) => spec.same({ ...entry.filter, ...kept() }, filter()));
  const bar = (): FilterBarProps => ({
    value: spec.toValue(filter()),
    dirty: spec.isFiltered(filter()),
    onChange: (next) => change(spec.fromValue(next, filter())),
    onReset: reset,
    quick: (spec.quick ?? []).map((entry) => ({ id: entry.id, label: t(entry.label), active: entry.id === activeQuick()?.id })),
    // A quick filter replaces the whole filter, search included, but keeps what the spec keeps; the one in
    // effect takes itself away again.
    onQuick: (id) => {
      const chosen = spec.quick?.find((entry) => entry.id === id);
      if (!chosen) return;
      replaceAll({ ...(activeQuick()?.id === id ? spec.empty : chosen.filter), ...kept() });
    },
  });
  return { filter, search, onSearchInput, change, replaceAll, reset, bar };
}

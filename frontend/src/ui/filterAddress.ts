/**
 * Helpers shared by the filter address modules of the Files and Activity pages: comma lists in the address,
 * canonical order of known values and the shape of a filter option and a quick filter.
 */

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

import { createEffect, createSignal, type Accessor } from "solid-js";

/**
 * A refresh returns fresh objects for rows that did not change. These helpers tell a real change
 * from a new copy, so a view can keep what it has and rebuild only what moved. Ids are ignored when
 * comparing: they are derived from the row and never carry a change of their own.
 */

const isIdKey = (key: string) => key === "id" || key.endsWith("_id");

/** Deep equality that ignores every `id` and `*_id` field, at any depth. */
export function sameIgnoringIds(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((value, index) => sameIgnoringIds(value, b[index]));
  }
  const left = Object.entries(a as Record<string, unknown>).filter(([key]) => !isIdKey(key));
  const right = b as Record<string, unknown>;
  const rightKeys = Object.keys(right).filter((key) => !isIdKey(key));
  return left.length === rightKeys.length && left.every(([key, value]) => key in right && sameIgnoringIds(value, right[key]));
}

/** The previous object when only ids changed, so nothing that reads it reacts; otherwise the new one. */
export function keepIfSame<T>(next: T, previous: T | undefined): T {
  return previous !== undefined && sameIgnoringIds(next, previous) ? previous : next;
}

/**
 * Carry a refreshed list over what is already shown: a row that did not change stays the very same
 * object, and a row that did keeps the id it had, so a view keyed on either neither rebuilds nor
 * loses its place. Rows are matched by `keyOf`; a missing or repeated key is left as it arrived.
 */
export function reuseUnchanged<T extends { id: string }>(fresh: T[], previous: T[], keyOf: (row: T) => string | undefined): T[] {
  const tally = (rows: T[]) => {
    const seen = new Map<string, number>();
    for (const row of rows) {
      const key = keyOf(row);
      if (key !== undefined) seen.set(key, (seen.get(key) ?? 0) + 1);
    }
    return seen;
  };
  const before = tally(previous);
  const after = tally(fresh);
  const known = new Map<string, T>();
  for (const row of previous) {
    const key = keyOf(row);
    if (key !== undefined && before.get(key) === 1) known.set(key, row);
  }
  return fresh.map((row) => {
    const key = keyOf(row);
    const old = key !== undefined && after.get(key) === 1 ? known.get(key) : undefined;
    if (!old) return row;
    return sameIgnoringIds(row, old) ? old : { ...row, id: old.id };
  });
}

/**
 * True once a resource has loaded for the current key. Unlike `!resource.loading` it stays true
 * while the same key is read again in the background, so a refresh never swaps the view for
 * "Loading", and it turns false again at once when the key changes.
 */
export function loadedFor(resource: { latest: unknown; loading: boolean }, key: Accessor<string>): Accessor<boolean> {
  const [loaded, setLoaded] = createSignal<string | undefined>();
  createEffect(() => {
    if (resource.latest !== undefined && !resource.loading) setLoaded(key());
  });
  return () => loaded() === key();
}

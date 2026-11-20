/**
 * Only complete domain keys that are unique within this list keep rows across new responses.
 * With missing or duplicate keys, object identity remains the safe fallback.
 */
export function uniqueRowKeys<T>(rows: readonly T[], keyOf: (row: T) => string | undefined): (row: T) => string | T {
  const counts = new Map<string, number>();
  for (const row of rows) {
    const key = keyOf(row);
    if (key !== undefined) counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return (row) => {
    const key = keyOf(row);
    return key !== undefined && counts.get(key) === 1 ? key : row;
  };
}

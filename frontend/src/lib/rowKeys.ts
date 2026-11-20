/**
 * Nur vollständige, in dieser Liste eindeutige Fachschlüssel erhalten Zeilen über neue Antworten hinweg.
 * Bei fehlenden oder doppelten Schlüsseln bleibt Objektidentität die sichere Rückfallebene.
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

type Store = Pick<Storage, "getItem" | "setItem">;

/**
 * True the first time a moment is asked about on a given day. A pleasure that repeats on every visit
 * turns into noise, so each one is shown once a day and plain after that. Never throws.
 */
export function celebrateToday(moment: string, storage?: Store, now: Date = new Date()): boolean {
  try {
    const store = storage ?? window.localStorage;
    const day = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
    const slot = `atvprep:celebrated:${moment}`;
    if (store.getItem(slot) === day) return false;
    store.setItem(slot, day);
    return true;
  } catch {
    return false;
  }
}

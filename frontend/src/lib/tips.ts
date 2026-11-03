const slot = (name: string) => `atvprep:tip:${name}`;

/** Whether a one-time hint is still to be shown. Storage that is blocked just means it shows again. */
export function tipPending(name: string): boolean {
  try {
    return window.localStorage.getItem(slot(name)) !== "seen";
  } catch {
    return true;
  }
}

/** Remember that a hint did its job, so it is never shown twice. */
export function dismissTip(name: string): void {
  try {
    window.localStorage.setItem(slot(name), "seen");
  } catch {
    // Nothing to do: the hint may come back, which is better than failing.
  }
}

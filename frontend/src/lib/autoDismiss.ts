import { createEffect } from "solid-js";

/** A confirmation has done its job once it was read: clear it a while after it appeared. */
export function autoDismiss(message: () => string | null | undefined, clear: () => void, ms = 12000): void {
  createEffect(message, (value) => {
    if (!value) return;
    const timer = window.setTimeout(clear, ms);
    return () => window.clearTimeout(timer);
  });
}

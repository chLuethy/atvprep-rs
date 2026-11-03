import { createEffect, onCleanup } from "solid-js";

/** A confirmation has done its job once it was read: clear it a while after it appeared. */
export function autoDismiss(message: () => string | null | undefined, clear: () => void, ms = 12000): void {
  createEffect(() => {
    if (!message()) return;
    const timer = window.setTimeout(clear, ms);
    onCleanup(() => window.clearTimeout(timer));
  });
}

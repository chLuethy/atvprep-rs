import { createEffect, on, onCleanup } from "solid-js";

/** How long a changed element stays marked. Long enough to catch the eye, short enough not to nag. */
export const FLASH_MS = 800;

const timers = new WeakMap<HTMLElement, number>();

/**
 * Mark an element for a moment. The interface updates quietly in place, so without this a status
 * that flips from Warning to Ready is only noticed by whoever happens to be looking at it.
 */
export function flash(element: HTMLElement): void {
  const running = timers.get(element);
  if (running !== undefined) window.clearTimeout(running);
  element.classList.remove("flash");
  void element.offsetWidth; // restart the fade when it is flashed again while still lit
  element.classList.add("flash");
  timers.set(element, window.setTimeout(() => {
    element.classList.remove("flash");
    timers.delete(element);
  }, FLASH_MS));
}

/** A ref that flashes its element whenever `read()` changes, but not when the element first appears. */
export function flashOnChange(read: () => unknown): (element: HTMLElement) => void {
  return (element) => {
    createEffect(on(read, () => flash(element), { defer: true }));
    onCleanup(() => {
      const running = timers.get(element);
      if (running !== undefined) window.clearTimeout(running);
    });
  };
}

import { createSignal, getOwner, onCleanup, type Accessor } from "solid-js";

const [stamp, setStamp] = createSignal(Date.now());
let users = 0;
let timer: number | undefined;

const sync = () => { if (!document.hidden) setStamp(Date.now()); };

/**
 * The current time, ticking once a second for as long as something reads it. All readers share one
 * timer, and it stops with the last of them. A hidden tab does not tick; it catches up when seen.
 */
export function useClock(): Accessor<Date> {
  if (getOwner()) {
    if (users === 0) {
      setStamp(Date.now());
      timer = window.setInterval(sync, 1000);
      document.addEventListener("visibilitychange", sync);
    }
    users += 1;
    onCleanup(() => {
      users -= 1;
      if (users > 0) return;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", sync);
    });
  }
  return () => new Date(stamp());
}

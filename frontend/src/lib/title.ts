import { onCleanup, onMount } from "solid-js";
import { loadSummary } from "../features/files/loaders";
import { t } from "./i18n";
import { publishWork } from "./work";

export const APP_TITLE = "atvprep-manager";
const doneTitle = () => t("Done · {app}", { app: APP_TITLE });
const POLL_MS = 3000;

/** What the tab says while the manager works, so a tab left in the background still tells you. */
export function workTitle(work: { running: number; queued: number }): string {
  const parts = [work.running > 0 ? t("{count} running", { count: work.running }) : "", work.queued > 0 ? t("{count} waiting", { count: work.queued }) : ""].filter(Boolean);
  return parts.length > 0 ? `${parts.join(", ")} · ${APP_TITLE}` : APP_TITLE;
}

/**
 * Keep the tab title in step with the work. When everything ended while the tab was out of sight it
 * says "Done" until the tab is seen again, and then goes back to the plain name.
 */
export function useWorkTitle(api: { request: (path: string, init?: RequestInit) => Promise<Response> }): void {
  let busyBefore = false;
  let doneShown = false;
  let timer: number | undefined;

  const poll = async () => {
    const summary = await loadSummary(api);
    if (!summary) return;
    publishWork(summary.jobs);
    const busy = summary.jobs.running + summary.jobs.queued > 0;
    if (busy) {
      doneShown = false;
      document.title = workTitle(summary.jobs);
    } else if (busyBefore) {
      doneShown = document.hidden;
      document.title = doneShown ? doneTitle() : APP_TITLE;
    }
    busyBefore = busy;
  };
  const onVisible = () => {
    if (!document.hidden && doneShown) {
      doneShown = false;
      document.title = APP_TITLE;
    }
  };

  onMount(() => {
    void poll();
    timer = window.setInterval(() => void poll(), POLL_MS);
    document.addEventListener("visibilitychange", onVisible);
  });
  onCleanup(() => {
    window.clearInterval(timer);
    document.removeEventListener("visibilitychange", onVisible);
    document.title = APP_TITLE;
  });
}

import { onSettled } from "solid-js";
import { loadSummary } from "../features/files/loaders";
import { t } from "./i18n";
import { publishWork } from "./work";

export const APP_TITLE = "atvprep-manager";
const POLL_MS = 3000;

/** The name of the open route and the state of the work; together they make the tab title. */
let routeName = "";
let workPrefix = "";
const render = () => {
  document.title = [workPrefix, routeName, APP_TITLE].filter(Boolean).join(" · ");
};

/** The route the tab shows ("Files · atvprep-manager"); an empty name leaves the plain title. */
export function setRouteTitle(name: string): void {
  routeName = name;
  render();
}

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
      workPrefix = workTitle(summary.jobs).replace(` · ${APP_TITLE}`, "");
      render();
    } else if (busyBefore) {
      doneShown = document.hidden;
      workPrefix = doneShown ? t("Done") : "";
      render();
    }
    busyBefore = busy;
  };
  const onVisible = () => {
    if (!document.hidden && doneShown) {
      doneShown = false;
      workPrefix = "";
      render();
    }
  };

  onSettled(() => {
    void poll();
    timer = window.setInterval(() => void poll(), POLL_MS);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
      workPrefix = "";
      render();
    };
  });
}

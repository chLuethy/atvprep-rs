import { onSettled } from "solid-js";
import { loadSummary } from "../features/files/loaders";
import { t } from "./i18n";
import { publishWork } from "./work";

export const APP_TITLE = "atvprep-manager";
const POLL_MS = 3000;

/** The name of the open route and the state of the work; together they make the tab title. */
let routeName = "";
/** What the work is: counts while it runs, "done" once it ended unseen, nothing otherwise. Worded when the title is built, so a language change shows. */
type Work = { running: number; queued: number } | "done" | null;
let work: Work = null;
const workLabel = (state: Work): string => {
  if (state === "done") return t("Done");
  if (!state) return "";
  return [state.running > 0 ? t("{count} running", { count: state.running }) : "", state.queued > 0 ? t("{count} waiting", { count: state.queued }) : ""].filter(Boolean).join(", ");
};
const render = () => {
  document.title = [workLabel(work), routeName, APP_TITLE].filter(Boolean).join(" · ");
};

/** The route the tab shows ("Files · atvprep-manager"); an empty name leaves the plain title. Call again when the language changes. */
export function setRouteTitle(name: string): void {
  routeName = name;
  render();
}

/** What the tab says while the manager works, so a tab left in the background still tells you. */
export function workTitle(state: { running: number; queued: number }): string {
  return [workLabel(state), APP_TITLE].filter(Boolean).join(" · ");
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
      work = { running: summary.jobs.running, queued: summary.jobs.queued };
      render();
    } else if (busyBefore) {
      doneShown = document.hidden;
      work = doneShown ? "done" : null;
      render();
    }
    busyBefore = busy;
  };
  const onVisible = () => {
    if (!document.hidden && doneShown) {
      doneShown = false;
      work = null;
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
      work = null;
      render();
    };
  });
}

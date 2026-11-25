import { t } from "../../lib/i18n";
import type { Command } from "../../ui/CommandPalette";
import { jobTitle } from "../activity/model";
import { EMPTY_JOB_FILTER } from "../activity/filters";
import { loadJobs } from "../activity/loaders";
import { EMPTY_FILTER } from "../files/filters";
import { loadFiles } from "../files/loaders";
import { baseName } from "../files/model";
import { loadSchedules } from "../schedules/loaders";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

/** How many hits each kind adds to the palette; the lists themselves are the place for more. */
const PER_KIND = 8;

/**
 * Hits for a typed query from the reads the lists already use: files and jobs by the server's search (name,
 * so also the episode tag and title), schedules by name in the client, as the schedule list itself filters.
 * A read that fails adds nothing; the other kinds still show.
 */
export async function searchTargets(api: Api, query: string, go: (path: string) => void): Promise<Command[]> {
  const needle = query.trim();
  if (!needle) return [];
  const lower = needle.toLowerCase();
  const [files, jobs, schedules] = await Promise.all([
    loadFiles(api, { ...EMPTY_FILTER, q: needle, cursor: null }).catch(() => null),
    loadJobs(api, { filter: { ...EMPTY_JOB_FILTER, q: needle } }).catch(() => null),
    loadSchedules(api).catch(() => null),
  ]);
  // The server lists newest first; equally good matches read better in name order (S01E01 before S01E02).
  const named = (files && !files.error ? files.items : []).map((file) => ({ file, name: baseName(file.display_path ?? "").title || file.id }));
  named.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
  const fileHits: Command[] = named.slice(0, PER_KIND).map(({ file, name }) => ({
    id: `file-${file.id}`,
    label: name,
    hint: t("File"),
    run: () => go(`/files/${encodeURIComponent(file.id)}`),
  }));
  const jobHits: Command[] = (jobs?.kind === "ok" ? jobs.items : []).slice(0, PER_KIND).map((job) => ({
    id: `job-${job.id}`,
    // A job found by a later file names that file first, so the hit shows why it matched.
    label: job.matched_path ? `${baseName(job.matched_path).title} · ${jobTitle(job)}` : jobTitle(job),
    hint: t("Job"),
    run: () => go(`/jobs/${encodeURIComponent(job.id)}`),
  }));
  const scheduleHits: Command[] = (schedules?.kind === "ok" ? schedules.items : [])
    .filter((schedule) => schedule.name.toLowerCase().includes(lower))
    .slice(0, PER_KIND)
    .map((schedule) => ({ id: `schedule-${schedule.id}`, label: schedule.name, hint: t("Schedule"), run: () => go("/schedules") }));
  return [...fileHits, ...jobHits, ...scheduleHits];
}

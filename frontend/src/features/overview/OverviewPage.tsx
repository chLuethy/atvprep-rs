import { createMemo, createSignal, Errored, For, Loading, refresh, Show, type Component } from "solid-js";
import type { JSX } from "@solidjs/web";
import { Button, StatusChip } from "../../ui";
import { actionLabel, jobProgress, jobTitle, jobTone } from "../activity/model";
import { JOB_QUICK_FILTERS, jobFilterQuery } from "../activity/filters";
import type { JobRow } from "../activity/loaders";
import { ScanControl } from "../scan/ScanControl";
import { Orphans, type OrphanHit } from "./Orphans";
import { N, t, tn } from "../../lib/i18n";
import { celebrateToday } from "../../lib/celebrate";
import { verdictOf } from "../files/model";

type OverviewApi = { request: (path: string, init?: RequestInit) => Promise<Response> };

type Overview = {
  orphans: OrphanHit[];
  files: Record<string, number>;
  jobs: Record<string, number>;
  encodeAudio: number;
  encodeVideo: number;
  activity: Array<{ id: string; topic: string }>;
  queue: { total: number; jobs: JobRow[] };
};

async function load(api: OverviewApi): Promise<Overview> {
  const responses = await Promise.all([api.request("/dashboard"), api.request("/dashboard/activity?limit=8"), api.request("/dashboard/queue?limit=5")]);
  if (responses.some((response) => !response.ok)) throw new Error("overview unavailable");
  const [summary, activity, queue] = await Promise.all(responses.map((response) => response.json()));
  return {
    files: summary.data?.files ?? {},
    jobs: summary.data?.jobs ?? {},
    encodeAudio: summary.data?.encode_audio ?? 0,
    encodeVideo: summary.data?.encode_video ?? 0,
    orphans: summary.data?.orphans ?? [],
    activity: activity.data ?? [],
    queue: { total: queue.data?.total ?? 0, jobs: queue.data?.jobs ?? [] },
  };
}

/** The states in the order of the health panel. The badge carries the status; the bar is only a share. */
const STATES: ReadonlyArray<{ state: string; label: string }> = [
  { state: "clean", label: N("Ready") },
  { state: "embed_open", label: N("Embedding open") },
  { state: "warning", label: N("Warning") },
  { state: "blocker", label: N("Blocker") },
  { state: "unknown", label: N("Not verified") },
  { state: "probe_failed", label: N("Inspection failed") },
];

/** Symbol of a "Needs you" row: carries the kind without colour alone. */
const SYMBOLS = { bad: "\u2715", warn: "\u25B2", neutral: "\u2013" } as const;
type Tone = keyof typeof SYMBOLS;

const TOPICS: Record<string, string> = {
  "job.created": N("Job queued"),
  "job.retry_created": N("Retry queued"),
  "job.transition": N("Job changed state"),
  "job.item.transition": N("A file moved on"),
  "job.log": N("Log output"),
  "orphan.cleanup": N("Leftover files removed"),
};

/** The activity with the quick filter "Running": every job that waits, runs or is being cancelled, as the queue counts them. */
const RUNNING_JOBS = `/jobs?${jobFilterQuery(JOB_QUICK_FILTERS.find((entry) => entry.id === "running")!.filter)}`;

/** Rows for the "Needs you" list, worst first; each says what it is and leads to the place that fixes it. */
function attention(files: Record<string, number>, jobs: Record<string, number>): Array<{ text: string; href: string; tone: Tone }> {
  const count = (record: Record<string, number>, key: string) => record[key] ?? 0;
  const rows: Array<{ n: number; tone: Tone; text: (n: number) => string; href: string }> = [
    { n: count(files, "blocker"), tone: "bad", href: "/files?status=blocker", text: (n) => tn("{count} file has a blocker", "{count} files have blockers", n) },
    { n: count(files, "warning"), tone: "warn", href: "/files?status=warning", text: (n) => tn("{count} file has a warning", "{count} files have warnings", n) },
    { n: count(files, "unknown"), tone: "neutral", href: "/files?status=unknown", text: (n) => tn("{count} file is not verified", "{count} files are not verified", n) },
    { n: count(files, "probe_failed"), tone: "bad", href: "/files?status=probe_failed", text: (n) => tn("{count} file could not be inspected", "{count} files could not be inspected", n) },
    { n: count(jobs, "failed"), tone: "bad", href: "/jobs?status=failed", text: (n) => tn("{count} job failed", "{count} jobs failed", n) },
    { n: count(jobs, "partial_success"), tone: "warn", href: "/jobs?status=partial_success", text: (n) => tn("{count} job partly failed", "{count} jobs partly failed", n) },
  ];
  return rows.filter((row) => row.n > 0).map((row) => ({ text: row.text(row.n), href: row.href, tone: row.tone }));
}

/** Where to look first: what needs you, how healthy the files are, what is running. */
export const OverviewPage: Component<{ api: OverviewApi; onNavigate?: (href: string) => void }> = (props) => {
  const overview = createMemo(() => load(props.api));
  const refetch = () => refresh(overview);
  const [removed, setRemoved] = createSignal<string | null>(null);

  const link = (href: string, content: JSX.Element, className = "") => (
    <a
      class={className}
      data-health-row={className === "health-row" ? "" : undefined}
      href={href}
      onClick={(event) => {
        if (!props.onNavigate) return;
        event.preventDefault();
        props.onNavigate(href);
      }}
    >{content}</a>
  );

  return (
    <div class="overview">
      <h1>{t("Overview")}</h1>
      <Loading fallback={<p class="muted">{t("Loading overview")}</p>}>
        <Errored fallback={(_error, retry) => <div class="notice bad" role="alert"><p>{t("Unable to load overview")}</p><Button onClick={retry}>{t("Retry")}</Button></div>}>
        <Show
          when={overview()}
          fallback={<div class="notice bad" role="alert"><p>{t("Unable to load overview")}</p><Button onClick={() => void refetch()}>{t("Retry")}</Button></div>}
        >
          {(data) => {
            const total = () => STATES.reduce((sum, entry) => sum + (data().files[entry.state] ?? 0), 0);
            const spoken = () => `${tn("{count} file", "{count} files", total())}: ${STATES.filter((entry) => (data().files[entry.state] ?? 0) > 0)
              .map((entry) => `${data().files[entry.state]} ${t(entry.label).toLowerCase()}`).join(", ")}`;
            const rows = () => attention(data().files, data().jobs);
            return (
              <>
                <Show when={removed()}><p class="notice ok" role="status">{removed()}</p></Show>
                <div class="ov-cols">
                  <div class="ov-col">
                    <Show when={data().orphans.length > 0}>
                      <section class="ov-sec panel" role="region" aria-label={t("Leftover files")}>
                        <h2>{t("Leftover files")}</h2>
                        <For each={data().orphans} keyed={false}>{(hit) => (
                          <Orphans api={props.api} hit={hit()} onChanged={() => void refetch()} onDone={(message) => { setRemoved(message); void refetch(); }} />
                        )}</For>
                      </section>
                    </Show>

                    <section class="ov-sec panel" role="region" aria-label={t("Needs you")}>
                      <h2>{t("Needs you")}</h2>
                      <Show when={rows().length > 0} fallback={<p class="muted">{t("Nothing needs you.")}</p>}>
                        <ul class="plain needs">
                          <For each={rows()}>{(row) => <li><span class={`sym ${row.tone}`} aria-hidden="true">{SYMBOLS[row.tone]}</span>{link(row.href, row.text)}</li>}</For>
                        </ul>
                      </Show>
                    </section>

                    <section class="ov-sec panel" role="region" aria-label={t("Running now")}>
                      <h2>{t("Running now")}</h2>
                      <Show when={data().queue.jobs.length > 0} fallback={<p class="muted">{t("Nothing is running.")}</p>}>
                        <p class="muted">{link(RUNNING_JOBS, tn("{count} active job", "{count} active jobs", data().queue.total))}</p>
                        <ul class="plain now">
                          <For each={data().queue.jobs}>{(job) => {
                            const tone = () => jobTone(job.status);
                            return (
                              <li class="now-row">
                                {link(`/jobs/${encodeURIComponent(job.id)}`, `${job.subject ? `${actionLabel(job.action)}: ${jobTitle(job)}` : actionLabel(job.action)} · ${jobProgress(job)}`)}
                                <StatusChip kind={tone().kind} icon={tone().icon}>{tone().label}</StatusChip>
                              </li>
                            );
                          }}</For>
                        </ul>
                      </Show>
                    </section>
                  </div>

                  <div class="ov-col">
                    <section class="ov-sec panel" role="region" aria-label={t("File health")}>
                      <h2>{t("File health")}</h2>
                      <Show when={total() > 0} fallback={<><p class="muted">{t("No files yet: scan a library to find them.")}</p><ScanControl api={props.api} onFinished={() => void refetch()} /></>}>
                        <Show when={(data().files.clean ?? 0) === total()}>
                          {(() => {
                            // Once a day the check draws itself; every other time it is simply there.
                            const draw = celebrateToday("all-ready");
                            return (
                              <p class="ready">
                                <svg class={["ready-check", { draw }]} aria-hidden="true" viewBox="0 0 16 16" width="16" height="16"><path d="M3 8.5l3.2 3.2L13 4.8" /></svg>
                                {total() === 1 ? t("The file is ready for Apple TV.") : t("All {count} files are ready for Apple TV.", { count: total() })}
                              </p>
                            );
                          })()}
                        </Show>
                        <span class="sr-only" role="img" aria-label={spoken()} />
                        <div class="health">
                          <For each={STATES.filter((entry) => (data().files[entry.state] ?? 0) > 0)}>{(entry) => {
                            const verdict = verdictOf({ state: entry.state });
                            const share = () => ((data().files[entry.state] ?? 0) / total()) * 100;
                            return link(`/files?status=${entry.state}`, (
                              <>
                                <span class="count">{data().files[entry.state]}</span>{" "}
                                <StatusChip kind={verdict.kind} icon={verdict.icon}>{t(entry.label)}</StatusChip>
                                <span class={["hbar", { neutral: verdict.kind === "neutral" }]} data-health-bar aria-hidden="true"><span style={`width:${share()}%`} /></span>
                              </>
                            ), "health-row");
                          }}</For>
                        </div>
                        <Show when={data().encodeVideo > 0 || data().encodeAudio > 0}>
                          <ul class="plain legend">
                            <Show when={data().encodeVideo > 0}>
                              <li>{link("/files?has=encode_video", <><span class="count">{data().encodeVideo}</span> {t("Need video encoding")}</>)}</li>
                            </Show>
                            <Show when={data().encodeAudio > 0}>
                              <li>{link("/files?has=encode_audio", <><span class="count">{data().encodeAudio}</span> {t("Need audio encoding")}</>)}</li>
                            </Show>
                          </ul>
                        </Show>
                      </Show>
                    </section>
                  </div>
                </div>

                <section class="ov-sec panel" role="region" aria-label={t("Recent activity")}>
                  <h2>{t("Recent activity")}</h2>
                  <Show when={data().activity.length > 0} fallback={<p class="muted">{t("No activity yet")}</p>}>
                    <ul class="plain feed"><For each={data().activity}>{(event) => <li>{TOPICS[event.topic] ? t(TOPICS[event.topic]!) : event.topic}</li>}</For></ul>
                  </Show>
                </section>
              </>
            );
          }}
        </Show>
        </Errored>
      </Loading>
    </div>
  );
};

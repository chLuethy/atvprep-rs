import { createEffect, createMemo, createSignal, For, Loading, Match, onSettled, refresh, Show, Switch, type Accessor, type Component } from "solid-js";
import { SessionError } from "../../lib/api";
import { Button, Icon, Kbd, StatusChip } from "../../ui";
import { loadDetailMounted, loadInspectionsMounted, loadJobsMounted, queueJob, rawId, type Api, type FileDetail, type Inspection } from "./loaders";
import {
  baseName,
  formatBitrate,
  formatDuration,
  formatSize,
  languageName,
  parseReason,
  summarizeInspection,
  verdictOf,
  wasJustFixed,
  type ParsedReason,
} from "./model";
import { isMp4Like } from "./container";
import { changeLabel, needLabel, prepareConfirm, preparable } from "./preparePlan";
import { createEncodeChoice, EncodeChoice } from "./EncodeChoice";
import { LIMIT_OPTIONS_MBPS, readWaveLimit, waveVerdict, writeWaveLimit } from "./playback";
import { Sidecars } from "./Sidecars";

import { formatDateTime } from "../../lib/time";
import { N, t } from "../../lib/i18n";

const SESSION_EXPIRED = N("Session expired. Lock and unlock again.");

/** The button that stands in for a preparation that is still going. */
const BUSY: Record<string, string> = {
  "Prepare queued": N("Prepare queued"), "Prepare running": N("Prepare running"), "Prepare cancelling": N("Prepare cancelling"),
};

function freshness(inspection: Inspection, lastSuccessAt: string | null): string {
  if (!inspection.created_at || !lastSuccessAt) return t("Freshness unavailable");
  const created = new Date(inspection.created_at).getTime();
  const verified = new Date(lastSuccessAt).getTime();
  if (Number.isNaN(created) || Number.isNaN(verified)) return t("Freshness unavailable");
  return created < verified ? t("Stale inspection") : t("Inspection current");
}

const formatDate = (iso: string | null) => formatDateTime(iso, t("Unknown"));

const Reason: Component<{ reason: ParsedReason; state: string }> = (props) => (
  <li class="why-item">
    <Show
      when={props.reason.severity}
      fallback={<Icon name={props.state === "clean" ? "check" : "dot"} size={14} />}
    >
      {(severity) => (
        <StatusChip kind={severity() === "blocker" ? "bad" : "warn"} icon={severity() === "blocker" ? "x" : "warn"}>
          {severity()}
        </StatusChip>
      )}
    </Show>
    <div>
      <Show when={props.reason.rule}><span class="mono rule">{props.reason.rule}</span></Show>
      <p>{props.reason.message}</p>
    </div>
  </li>
);

/** Everything the manager knows about one file: verdict and reasons first, evidence below. */
const InspectorView: Component<{
  api: Api;
  fileId: string;
  onFix?: (file: FileDetail) => void;
  onVerify?: (id: string) => void;
  /** The setting „Delete original“, which decides what the confirmation says about another container; null when unknown. */
  deleteOriginal?: boolean | null;
  /** Bumped by the page after it queued something, so the file is read again right away. */
  version?: number;
  /** Told after this view queued a job itself (a re-inspection), so the page can follow the queue. */
  onQueued?: () => void;
}> = (props) => {
  const detail = createMemo(() => loadDetailMounted(props.api, props.fileId));
  const inspections = createMemo(() => loadInspectionsMounted(props.api, props.fileId), { loadingValue: undefined });
  const jobs = createMemo(() => loadJobsMounted(props.api, props.fileId), { loadingValue: undefined });
  const reload = () => refresh(detail);
  const refetchInspections = () => refresh(inspections);
  const refetchJobs = () => refresh(jobs);
  createEffect(() => props.version, () => {
    void reload(); void refetchInspections(); void refetchJobs();
  }, { defer: true });

  const [limit, setLimit] = createSignal(readWaveLimit());
  const [confirming, setConfirming] = createSignal(false);
  const [inspecting, setInspecting] = createSignal(false);
  const [message, setMessage] = createSignal<string | null>(null);
  const [inspectError, setInspectError] = createSignal(false);
  const [sessionExpired, setSessionExpired] = createSignal(false);
  const [converting, setConverting] = createSignal(false);
  const [convertBusy, setConvertBusy] = createSignal(false);
  const [convertError, setConvertError] = createSignal<string | null>(null);
  let convertKey = crypto.randomUUID();
  let idempotencyKey: string | null = null;
  let trigger: HTMLButtonElement | undefined;
  let confirmButton: HTMLButtonElement | undefined;
  let wasConfirming = false;
  let inspectInFlight = false;
  let convertInFlight = false;
  let disposed = false;
  onSettled(() => () => { disposed = true; });

  createEffect(confirming, (open) => {
    if (open) queueMicrotask(() => confirmButton?.focus());
    else if (wasConfirming) queueMicrotask(() => trigger?.focus());
    wasConfirming = open;
  });
  // Switching files creates its own instance, including messages and confirmations.

  const openConfirm = () => {
    idempotencyKey ??= crypto.randomUUID();
    setMessage(null);
    setInspectError(false);
    setConfirming(true);
  };
  const closeConfirm = () => {
    setConfirming(false);
    idempotencyKey = null;
  };
  const reInspect = async () => {
    if (inspectInFlight) return;
    inspectInFlight = true;
    setInspecting(true);
    setInspectError(false);
    try {
      const response = await props.api.request(`/files/${props.fileId}/inspect`, {
        method: "POST",
        headers: { "Idempotency-Key": idempotencyKey ?? crypto.randomUUID() },
      });
      if (disposed) return;
      if (!response.ok) throw new Error("inspect failed");
      setMessage(t("Re-inspection queued"));
      closeConfirm();
      props.onQueued?.();
      void refetchJobs();
      void refetchInspections();
    } catch (error) {
      if (disposed) return;
      if (error instanceof SessionError) {
        setSessionExpired(true);
        closeConfirm();
      } else {
        setInspectError(true);
      }
    } finally {
      inspectInFlight = false;
      setInspecting(false);
    }
  };

  // Everything below reads the file through accessors, so a refresh updates just the parts whose
  // data moved instead of building the whole page again.
  const loaded = createMemo(() => detail());
  const file = createMemo(() => {
    const state = loaded();
    return state?.kind === "ok" ? state.data : undefined;
  });
  const failure = createMemo(() => {
    const state = loaded();
    return !state || state.kind === "error" ? "error" : state.kind === "session" ? "session" : undefined;
  });
  const reasonTexts = createMemo(() => file()?.compliance_reasons ?? [], {
    equals: (a, b) => a.length === b.length && a.every((text, index) => text === b[index]),
  });
  const reasons = createMemo(() => reasonTexts().map(parseReason));
  // Tracks come from the newest ffprobe inspection: a verify report is newer after a verify
  // but carries only violations, no streams.
  const latest = () => {
    const value = inspections();
    return value?.kind === "ok" ? value.items.find((item) => item.tool_name === "ffprobe") ?? value.items[0] : undefined;
  };
  const summary = () => {
    const inspection = latest();
    return inspection ? summarizeInspection(inspection.result) : undefined;
  };
  const confirm = (f: FileDetail) => prepareConfirm(f, props.deleteOriginal ?? null);
  const encode = createEncodeChoice();
  const submitConvert = async () => {
    if (convertInFlight || !encode.ready()) return;
    convertInFlight = true;
    setConvertBusy(true);
    setConvertError(null);
    // The id the server put in the file it sent, as Verify does: the one in the address may be URL-encoded.
    const id = file()?.id ?? rawId(props.fileId);
    const level = encode.level();
    const force = encode.force();
    const result = await queueJob(props.api, "prepare", [id], `${convertKey}-${level}-${force}`, { encode: level, confirmVideoEncode: level === "all", force });
    convertInFlight = false;
    setConvertBusy(false);
    if (disposed) return;
    if (!result.ok) {
      setConvertError(result.session ? t(SESSION_EXPIRED) : result.detail ? t("Unable to queue the preparation. The server said: {detail}.", { detail: result.detail }) : t("Unable to queue the preparation. Try again."));
      return;
    }
    setConverting(false);
    setMessage(t("Preparation queued. The new MP4 appears in Files after the next scan."));
    props.onQueued?.();
    void refetchJobs();
  };
  const mediaTracks = () => (summary()?.tracks ?? []).filter((track) => track.kind !== "other");

  const renderBody = () => (
    <Switch>
      <Match when={failure() === "error"}>
        <div class="notice bad" role="alert">
          <p>{t("Unable to load file detail")}</p>
          <Button onClick={() => void reload()}>{t("Retry")}</Button>
        </div>
      </Match>
      <Match when={failure() === "session"}>
        <p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p>
      </Match>
      <Match when={file()}>{(f) => renderFile(f)}</Match>
    </Switch>
  );

  const renderFile = (f: Accessor<FileDetail>) => {
    createEffect(() => f().fix_status, (status) => {
      if (!status) return;
      const timer = window.setInterval(() => { if (!document.hidden) void reload(); }, 4000);
      return () => window.clearInterval(timer);
    });
    const verdict = () => verdictOf(f());
    const name = () => baseName(f().display_path ?? "");
    return (
      <>
        <header class="insp-head">
          <h2>{name().title || t("{kind} file", { kind: f().kind })}</h2>
          <Show when={name().folder}><p class="mono muted">{name().folder}</p></Show>
          <div class="insp-verdict">
            <StatusChip kind={verdict().kind} icon={verdict().icon}>{verdict().label}</StatusChip>
          </div>
          <p class="muted insp-need">{t("Encoding need")}: <b>{needLabel(f().encode_need)}</b> · {t("Change")}: <b>{changeLabel(f().planned_change)}</b></p>
          <div class="insp-actions">
            <Show when={f().fix_status}>
              {(status) => <Button variant="primary" disabled title={t("A preparation for this file is already in progress.")}>{(() => { const label = `Prepare ${status()}`; return BUSY[label] ? t(BUSY[label]) : label; })()}</Button>}
            </Show>
            <Show when={!f().fix_status && !isMp4Like(f().suffix) && preparable(f())}>
              <Button variant="primary" aria-expanded={converting() ? "true" : "false"} onClick={() => { setConvertError(null); setConverting(!converting()); }}>{t("Prepare")}</Button>
            </Show>
            <Show when={!f().fix_status && props.onFix && isMp4Like(f().suffix) && preparable(f())}>
              <Button variant="primary" onClick={() => props.onFix?.(f())}>{t("Prepare preview")} <Kbd>F</Kbd></Button>
            </Show>
            <Show when={props.onVerify && isMp4Like(f().suffix)}>
              <Button variant={wasJustFixed(f()) ? "primary" : "default"} onClick={() => props.onVerify?.(f().id)}>{t("Verify")}</Button>
            </Show>
            <Button
              disabled={!f().inspection_available}
              aria-expanded={confirming() ? "true" : "false"}
              ref={(element) => { trigger = element; }}
              onClick={openConfirm}
            >
              {t("Re-inspect")}
            </Button>
          </div>
          <Show when={f().last_interrupted && !f().fix_status}>
            <p class="notice warn" role="status">
              {t("Last preparation interrupted")}
              <Show when={preparable(f()) && (isMp4Like(f().suffix) ? props.onFix : true)}>
                {" "}
                <Button variant="quiet" onClick={() => { if (isMp4Like(f().suffix)) props.onFix?.(f()); else { setConvertError(null); setConverting(true); } }}>{t("Prepare again")}</Button>
              </Show>
            </p>
          </Show>
          <Show when={converting()}>
            <div class="confirm" role="region" aria-label={t("Prepare")} onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); setConverting(false); } }}>
              <For each={confirm(f()).lines}>{(line, index) => <p class={confirm(f()).reencode && index() === 0 ? "notice warn" : undefined} role={confirm(f()).reencode && index() === 0 ? "alert" : undefined}>{line}</p>}</For>
              <EncodeChoice choice={encode} offerForce />
              <Show when={convertError()}><p class="notice bad" role="alert">{convertError()}</p></Show>
              <div class="insp-actions">
                <Button variant="primary" disabled={convertBusy() || !encode.ready()} onClick={() => void submitConvert()}>{t("Confirm prepare")}</Button>
                <Button variant="quiet" onClick={() => setConverting(false)}>{t("Cancel")}</Button>
              </div>
            </div>
          </Show>
          <Show when={confirming()}>
            <div
              class="confirm"
              role="region"
              aria-label={t("Confirm re-inspection")}
              onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); closeConfirm(); } }}
            >
              <p>{t("Queue a new ffprobe inspection for this file? Nothing is changed on disk.")}</p>
              <div class="insp-actions">
                <Button variant="primary" disabled={inspecting()} ref={(element) => { confirmButton = element; }} onClick={() => void reInspect()}>{t("Confirm re-inspect")}</Button>
                <Button variant="quiet" onClick={closeConfirm}>{t("Cancel")}</Button>
              </div>
            </div>
          </Show>
          <Show when={message()}><p class="notice ok" role="status">{message()}</p></Show>
          <Show when={inspectError()}><p class="notice bad" role="alert">{t("Unable to queue re-inspection")}</p></Show>
          <Show when={sessionExpired()}><p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p></Show>
          <dl class="facts">
            <div><dt>{t("Size")}</dt><dd>{formatSize(f().size)}</dd></div>
            <Show when={summary()?.durationSeconds}><div><dt>{t("Duration")}</dt><dd>{formatDuration(summary()?.durationSeconds)}</dd></div></Show>
            <Show when={summary()?.bitrate}><div><dt>{t("Bitrate")}</dt><dd>{formatBitrate(summary()?.bitrate)}</dd></div></Show>
            <Show when={summary()?.container}><div><dt>{t("Container")}</dt><dd class="mono">{summary()?.container}</dd></div></Show>
          </dl>
        </header>

        <section class="insp-sec" aria-labelledby="why-title">
          <h3 id="why-title">{t("Why")}</h3>
          <Show when={reasons().length > 0} fallback={<p class="muted">{t("No reason recorded.")}</p>}>
            <ul class="why" aria-label={t("Why")}>
              <For each={reasons()}>{(reason) => <Reason reason={reason} state={f().compliance_state} />}</For>
            </ul>
          </Show>
          <Show when={f().compliance_state === "unknown"}>
            <p class="muted">{t("Not verified: only a verify report can mark a file ready.")}</p>
          </Show>
        </section>

        <section class="insp-sec" aria-labelledby="tracks-title">
          <h3 id="tracks-title">{t("Tracks")}</h3>
          <Show when={inspections() !== undefined} fallback={<p class="muted">{t("Loading inspection")}</p>}>
            <Show when={inspections()?.kind !== "session"} fallback={<p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p>}>
              <Show when={!inspections()?.error} fallback={<div class="notice bad" role="alert"><p>{t("Unable to load inspections")}</p><Button onClick={() => void refetchInspections()}>{t("Retry")}</Button></div>}>
                <Show when={mediaTracks().length > 0} fallback={<p class="muted">{t("No tracks yet: the file has not been inspected.")}</p>}>
                  <div class="scroll-x">
                    <table class="tracks" aria-label={t("Tracks")}>
                      <thead><tr><th scope="col">{t("Track")}</th><th scope="col">{t("Language")}</th><th scope="col">{t("Codec")}</th><th scope="col">{t("Detail")}</th><th scope="col">{t("Flags")}</th></tr></thead>
                      <tbody>
                        <For each={mediaTracks()}>{(track) => (
                          <tr>
                            <th scope="row" class="mono">{track.label}</th>
                            <td>{track.kind === "video" ? "" : languageName(track.language)}</td>
                            <td>{track.codec}</td>
                            <td>{track.detail}</td>
                            <td>{[track.isDefault ? t("Default") : "", track.isForced ? t("Forced") : ""].filter(Boolean).join(" · ")}</td>
                          </tr>
                        )}</For>
                      </tbody>
                    </table>
                  </div>
                </Show>
                <p class="foot">{t("Apple tag status (elng, fall, folw, forc) comes from a verify report and is listed under Why.")}</p>
              </Show>
            </Show>
          </Show>
        </section>

        <section class="insp-sec" role="region" aria-label={t("Playback in Wave")}>
          <h3>{t("Playback in Wave")}</h3>
          <div class="wave-limit">
            <label for="wave-limit">{t("Your Wave limit")}</label>
            <select
              id="wave-limit"
              class="select"
              onChange={(event) => { const value = Number(event.currentTarget.value); setLimit(value); writeWaveLimit(value); }}
            >
              <For each={LIMIT_OPTIONS_MBPS}>{(option) => <option value={option} selected={option === limit()}>{`${option} Mbps`}</option>}</For>
            </select>
          </div>
          {(() => {
            const verdict = () => waveVerdict(summary()?.bitrate, limit());
            const chip = () => (verdict().state === "transcode"
              ? { kind: "warn" as const, icon: "transcode" as const }
              : verdict().state === "direct" ? { kind: "ok" as const, icon: "check" as const } : { kind: "neutral" as const, icon: "dot" as const });
            return (
              <>
                <p><StatusChip kind={chip().kind} icon={chip().icon}>{verdict().headline}</StatusChip></p>
                <p class="muted">{verdict().detail}</p>
              </>
            );
          })()}
        </section>

        <Show when={latest()}>
          {(inspection) => (
            <section class="insp-sec" aria-labelledby="inspection-title">
              <h3 id="inspection-title">{t("Inspection")}</h3>
              <p>
                <span>{inspection().tool_name} {inspection().tool_version}</span>
                <span class="muted"> · {formatDate(inspection().created_at)} · </span>
                <span>{freshness(inspection(), f().last_success_at)}</span>
              </p>
              <details>
                <summary>{t("Raw JSON")}</summary>
                <pre class="raw" aria-label={t("Raw inspection JSON")}>{JSON.stringify(inspection().result, null, 2)}</pre>
              </details>
            </section>
          )}
        </Show>

        <Sidecars api={props.api} mediaId={props.fileId} count={f().sidecar_count} version={props.version} />

        <section class="insp-sec" aria-labelledby="history-title">
          <h3 id="history-title">{t("Job history")}</h3>
          <Show when={jobs() !== undefined} fallback={<p class="muted">{t("Loading job history")}</p>}>
            {(() => {
              const value = jobs();
              if (!value) return null;
              if (value.kind === "session") return <p class="notice bad" role="alert">{t(SESSION_EXPIRED)}</p>;
              if (value.kind === "error") return <div class="notice bad" role="alert"><p>{t("Unable to load job history")}</p><Button onClick={() => void refetchJobs()}>{t("Retry")}</Button></div>;
              return (
                <Show when={value.items.length > 0} fallback={<p class="muted">{t("No job history")}</p>}>
                  <ul class="plain"><For each={value.items}>{(job) => <li>{`${job.action}: ${job.status} (${job.trigger})`}</li>}</For></ul>
                </Show>
              );
            })()}
          </Show>
        </section>
      </>
    );
  };

  return (
    <div class="inspector">
      <Loading fallback={<p class="muted">{t("Loading file detail")}</p>}>{renderBody()}</Loading>
    </div>
  );
};

export const Inspector: Component<Parameters<typeof InspectorView>[0]> = (props) => (
  <Show when={props.fileId} keyed>{(fileId) => <InspectorView {...props} fileId={fileId} />}</Show>
);

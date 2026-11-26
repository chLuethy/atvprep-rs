import { createMemo, For, Match, Show, Switch, type Component } from "solid-js";
import { Icon } from "../../ui";
import { t, tn } from "../../lib/i18n";
import { formatSize } from "../../lib/size";
import { actionLabel } from "./model";
import { parseLog, type FactKey, type LogRow, type RuleState } from "./logModel";

const factLabel = (key: FactKey): string => {
  switch (key) {
    case "Start": return t("Action");
    case "File": return t("File");
    case "Tool": return t("Tool");
    case "Command": return t("Command");
    case "Encode allowance": return t("Encode allowance");
    case "Target profile": return t("Target profile");
    case "Delete original": return t("Delete original");
    case "Force": return t("Force rebuild");
    case "State": return t("State");
    case "Encode need": return t("Encode need");
    case "Planned change": return t("Planned change");
  }
};

/** What a stream line's parts say: channels, sample rate and language each get a column, the rest are flags. */
function streamCells(stream: StreamRow) {
  let channels = "", rate = "", language = "";
  const flags: string[] = [];
  for (const part of stream.parts) {
    const c = /^(\d+) channels?$/.exec(part), r = /^(\d+) Hz$/.exec(part), l = /^language (.+)$/.exec(part);
    if (c) channels = c[1]!; else if (r) rate = part; else if (l) language = l[1]!; else flags.push(part);
  }
  return { channels, rate, language, flags };
}

/** The facts' values are the tool's own words except the yes/no a log spells out. */
const factValue = (key: FactKey, value: string): string => {
  if (key === "Start") return actionLabel(value);
  if (key === "Delete original" || key === "Force") return value === "yes" ? t("Yes") : value === "no" ? t("No") : value;
  return value;
};

type StreamRow = Extract<LogRow, { kind: "stream" }>;
type Shown = LogRow | { kind: "rules-ok"; count: number; time?: undefined } | { kind: "streams"; streams: StreamRow[]; time?: undefined };

/** Rules that passed are one line; a rule that did not stays a line of its own, in its place. */
function summarise(rows: readonly LogRow[]): Shown[] {
  const okCount = rows.filter((row) => row.kind === "rule" && row.state === "ok").length;
  const shown: Shown[] = [];
  let placed = false;
  let streams: Extract<Shown, { kind: "streams" }> | undefined;
  const errors = rows.filter((row): row is Extract<LogRow, { kind: "error" }> => row.kind === "error");
  for (const row of rows) {
    // The closing "Failed: code: message" line repeats the error the tool reported before it.
    if (row.kind === "error" && errors.some((other) => other !== row && other.text.length > row.text.length && other.text.includes(row.text))) continue;
    if (row.kind === "stream") {
      if (!streams) { streams = { kind: "streams", streams: [] }; shown.push(streams); }
      streams.streams.push(row);
      continue;
    }
    streams = undefined;
    if (row.kind === "rule" && row.state === "ok") {
      if (!placed) { shown.push({ kind: "rules-ok", count: okCount }); placed = true; }
      continue;
    }
    shown.push(row);
  }
  return shown;
}

const toneOf = (row: Shown): "ok" | "warn" | "bad" | "" => {
  switch (row.kind) {
    case "check": return row.passed ? "ok" : "bad";
    case "rule": return row.state === "blocker" ? "bad" : row.state === "warning" ? "warn" : "";
    case "error": return "bad";
    default: return "";
  }
};

const ruleLabel = (state: RuleState) => (state === "blocker" ? t("Blocker") : state === "warning" ? t("Warning") : t("Check"));

/**
 * One item's log, set out: the opening facts, the steps in order with their times and how the run
 * ended. The progress is not repeated here: the item row above already shows it while the item runs. The text it comes from is the same text "Download log" hands out.
 */
export const LogView: Component<{ text: string; itemId: string }> = (props) => {
  const doc = createMemo(() => parseLog(props.text));
  const rows = createMemo(() => summarise(doc().rows).map((row, index) => ({ row, key: `${index}:${JSON.stringify(row)}` })));
  const facts = () => doc().facts.filter((fact) => fact.key !== "Command");
  const command = () => doc().facts.find((fact) => fact.key === "Command")?.value;
  return (
    <div class="log-view" role="region" aria-label={t("Formatted log")}>
      <div class="log-head">
      <Show when={facts().length > 0}>
        <dl class="log-facts">
          <For each={facts()} keyed={(fact) => `${fact.key}:${fact.value}`}>{(factOf) => {
            const fact = factOf();
            return <div><dt class="label">{factLabel(fact.key)}</dt><dd>{factValue(fact.key, fact.value)}</dd></div>;
          }}</For>
        </dl>
      </Show>
      <Show when={command()}>{(value) => <details class="log-command"><summary>{t("Command")}</summary><code class="mono">{value()}</code></details>}</Show>
      </div>
      <div class="log-timeline">
      <Show when={rows().length > 0}>
        <ol class="log-steps">
          <For each={rows()} keyed={(entry) => entry.key}>{(entry) => {
            const row = entry().row;
            return (
            <li class={["log-step", row.kind, row.kind === "rule" ? row.state : "", row.kind === "note" && row.bullet ? "bullet" : "", toneOf(row) && `tone-${toneOf(row)}`, row.kind === "end" && row.exit === "0" ? "clean" : ""]}>
              <Switch>
                <Match when={row.kind === "phase" ? row : undefined}>{(phase) => (
                  <>
                    <Icon name="clock" />
                    <span class="log-main"><b>{t("Phase {name}", { name: phase().name })}</b>
                      <span class="muted">{[phase().took, phase().started ? t("started {time}", { time: phase().started ?? "" }) : "", phase().bytes ? formatSize(phase().bytes ?? 0) : ""].filter(Boolean).join(" · ")}</span></span>
                  </>
                )}</Match>
                <Match when={row.kind === "plan" ? row : undefined}>{(plan) => (
                  <><Icon name="transcode" /><span class="log-main"><b>{plan().stream || t("Plan")}</b><span>{plan().action}</span></span></>
                )}</Match>
                <Match when={row.kind === "rules-ok" ? row : undefined}>{(ok) => (
                  <><Icon name="check" /><span class="log-main">{tn("{count} rule passed", "{count} rules passed", ok().count)}</span></>
                )}</Match>
                <Match when={row.kind === "rule" ? row : undefined}>{(rule) => (
                  <><span class="log-mark" role="img" aria-label={ruleLabel(rule().state)}><Icon name={rule().state === "ok" ? "check" : "warn"} /></span><span class="log-main"><b class="mono">{rule().name}</b><Show when={rule().track}><span class="muted">{t("track {id}", { id: rule().track ?? "" })}</span></Show><Show when={rule().detail}><span>{rule().detail}</span></Show></span></>
                )}</Match>
                <Match when={row.kind === "streams" ? row : undefined}>{(block) => (
                  <div class="log-streams">
                    <span class="label">{t("Streams")}</span>
                    <div class="scroll-x">
                      <table class="tracks">
                        <thead><tr>
                          <th scope="col">#</th><th scope="col">{t("Type")}</th><th scope="col">{t("Codec")}</th><th scope="col">{t("Channels")}</th><th scope="col">{t("Sample rate")}</th><th scope="col">{t("Language")}</th><th scope="col">{t("Flags")}</th>
                        </tr></thead>
                        <tbody>
                          <For each={block().streams} keyed={(stream) => `${stream.index}:${stream.type}:${stream.codec}:${stream.parts.join("|")}`}>{(streamOf) => {
                            const stream = streamOf();
                            const cells = streamCells(stream);
                            return <tr><th scope="row" class="num c-idx">{stream.index}</th><td class="c-type">{stream.type}</td><td class="mono c-codec">{stream.codec}</td><td class="num c-ch" data-unit={t("ch")}>{cells.channels}</td><td class="num c-rate">{cells.rate}</td><td class="c-lang">{cells.language}</td><td class="c-flags">{cells.flags.join(", ")}</td></tr>;
                          }}</For>
                        </tbody>
                      </table>
                    </div>
                  </div>
                )}</Match>
                <Match when={row.kind === "check" ? row : undefined}>{(check) => (
                  <>
                    <Icon name={check().passed ? "check" : "x"} />
                    <span class="log-main"><b>{check().passed ? t("Check passed") : t("Check failed")}</b><span class="muted">{[tn("{count} blocker", "{count} blockers", check().blockers), tn("{count} warning", "{count} warnings", check().warnings)].join(" · ")}</span></span>
                  </>
                )}</Match>
                <Match when={row.kind === "outcome" ? row : undefined}>{(outcome) => (
                  <><Icon name="tag" /><span class="log-main"><b>{t("Outcome")}</b><span>{outcome().value}</span></span></>
                )}</Match>
                <Match when={row.kind === "original" ? row : undefined}>{(original) => (
                  <><Icon name="file" /><span class="log-main"><b>{t("Original")}</b><span>{original().kept ? t("kept") : t("removed")}{original().why ? ` (${original().why})` : ""}</span></span></>
                )}</Match>
                <Match when={row.kind === "error" ? row : undefined}>{(error) => (
                  <><Icon name="warn" /><span class="log-main bad-text">{error().text}</span></>
                )}</Match>
                <Match when={row.kind === "end" ? row : undefined}>{(end) => (
                  <><Icon name="dot" /><span class="log-main"><Show when={end().exit !== undefined}><b>{t("Exit code {code}", { code: end().exit ?? "" })}</b></Show><Show when={end().duration}><span class="muted">{`${t("Duration")} ${end().duration}`}</span></Show></span></>
                )}</Match>
                <Match when={row.kind === "omitted" ? row : undefined}>{(omitted) => (
                  <><Icon name="dot" /><span class="log-main muted">{t("{size} left out of the stored log", { size: formatSize(omitted().bytes) })}</span></>
                )}</Match>
                <Match when={row.kind === "note" ? row : undefined}>{(note) => (
                  <><Show when={!note().bullet} fallback={<span class="log-bullet" aria-hidden="true">•</span>}><Icon name="dot" /></Show><span class="log-main">{note().text}</span></>
                )}</Match>
              </Switch>
              <Show when={row.time}>{(time) => <time class="log-time">{time()}</time>}</Show>
            </li>
            );
          }}</For>
        </ol>
      </Show>
      </div>
      <Show when={rows().length === 0 && facts().length === 0}><p class="muted">{t("The log is empty.")}</p></Show>
    </div>
  );
};

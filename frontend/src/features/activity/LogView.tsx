import { createMemo, For, Match, Show, Switch, type Component } from "solid-js";
import { Icon, ProgressBar, StatusChip } from "../../ui";
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

/** The facts' values are the tool's own words except the yes/no a log spells out. */
const factValue = (key: FactKey, value: string): string => {
  if (key === "Start") return actionLabel(value);
  if (key === "Delete original" || key === "Force") return value === "yes" ? t("Yes") : value === "no" ? t("No") : value;
  return value;
};

type Shown = LogRow | { kind: "rules-ok"; count: number };

/** Rules that passed are one line; a rule that did not stays a line of its own, in its place. */
function summarise(rows: readonly LogRow[]): Shown[] {
  const okCount = rows.filter((row) => row.kind === "rule" && row.state === "ok").length;
  const shown: Shown[] = [];
  let placed = false;
  const errors = rows.filter((row): row is Extract<LogRow, { kind: "error" }> => row.kind === "error");
  for (const row of rows) {
    // The closing "Failed: code: message" line repeats the error the tool reported before it.
    if (row.kind === "error" && errors.some((other) => other !== row && other.text.length > row.text.length && other.text.includes(row.text))) continue;
    if (row.kind === "rule" && row.state === "ok") {
      if (!placed) { shown.push({ kind: "rules-ok", count: okCount }); placed = true; }
      continue;
    }
    shown.push(row);
  }
  return shown;
}

const ruleChip = (state: RuleState) => (state === "blocker" ? "bad" : state === "warning" ? "warn" : "neutral") as "bad" | "warn" | "neutral";
const ruleLabel = (state: RuleState) => (state === "blocker" ? t("Blocker") : state === "warning" ? t("Warning") : t("Check"));

/**
 * One item's log, set out: the opening facts, the steps in order with their times, the progress as one
 * value and how the run ended. The text it comes from is the same text "Download log" hands out.
 */
export const LogView: Component<{ text: string; itemId: string }> = (props) => {
  const doc = createMemo(() => parseLog(props.text));
  const rows = createMemo(() => summarise(doc().rows).map((row, index) => ({ row, key: `${index}:${JSON.stringify(row)}` })));
  const facts = () => doc().facts.filter((fact) => fact.key !== "Command");
  const command = () => doc().facts.find((fact) => fact.key === "Command")?.value;
  return (
    <div class="log-view" role="region" aria-label={t("Formatted log")}>
      <Show when={facts().length > 0}>
        <dl class="log-facts">
          <For each={facts()} keyed={(fact) => `${fact.key}:${fact.value}`}>{(factOf) => {
            const fact = factOf();
            return <div><dt class="label">{factLabel(fact.key)}</dt><dd>{factValue(fact.key, fact.value)}</dd></div>;
          }}</For>
        </dl>
      </Show>
      <Show when={command()}>{(value) => <details class="log-command"><summary>{t("Command")}</summary><code class="mono">{value()}</code></details>}</Show>
      <Show when={doc().progress !== undefined}>
        <div class="log-progress">
          <ProgressBar value={doc().progress ?? 0} track={`log:${props.itemId}`} label={t("Progress reported by the tool")} />
          <span class="num muted">{`${doc().progress}%`}</span>
        </div>
      </Show>
      <Show when={rows().length > 0}>
        <ol class="log-steps">
          <For each={rows()} keyed={(entry) => entry.key}>{(entry) => {
            const row = entry().row;
            return (
            <li class={["log-step", row.kind, row.kind === "rule" ? row.state : "", row.kind === "note" && row.bullet ? "bullet" : ""]}>
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
                  <><StatusChip kind={ruleChip(rule().state)} icon={rule().state === "ok" ? "check" : "warn"}>{ruleLabel(rule().state)}</StatusChip><span class="log-main"><b class="mono">{rule().name}</b><Show when={rule().track}><span class="muted">{t("track {id}", { id: rule().track ?? "" })}</span></Show><Show when={rule().detail}><span>{rule().detail}</span></Show></span></>
                )}</Match>
                <Match when={row.kind === "stream" ? row : undefined}>{(stream) => (
                  <>
                    <Icon name="rows" />
                    <span class="log-main"><b>{t("Stream {index}", { index: stream().index })}</b><span>{`${stream().type} ${stream().codec}`}</span>
                      <For each={stream().parts}>{(part) => <span class="log-tag">{part}</span>}</For></span>
                  </>
                )}</Match>
                <Match when={row.kind === "check" ? row : undefined}>{(check) => (
                  <>
                    <StatusChip kind={check().passed ? "ok" : "bad"} icon={check().passed ? "check" : "x"}>{check().passed ? t("Check passed") : t("Check failed")}</StatusChip>
                    <span class="log-main muted">{[tn("{count} blocker", "{count} blockers", check().blockers), tn("{count} warning", "{count} warnings", check().warnings)].join(" · ")}</span>
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
            </li>
            );
          }}</For>
        </ol>
      </Show>
      <Show when={rows().length === 0 && facts().length === 0}><p class="muted">{t("The log is empty.")}</p></Show>
    </div>
  );
};

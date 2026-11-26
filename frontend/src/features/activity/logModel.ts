/**
 * Reads the stored text log of one job item into parts the page can set: the opening facts, the steps in
 * order, the progress and what the run ended with. The text itself is never changed; a line that is not
 * recognised stays a plain note, so nothing the log says is lost.
 *
 * The text is what the manager stores (`[HH:MM:SS] step line` for its own steps, raw tool output between
 * them); see the worker's log module for what writes it.
 */

/** A fact the log opens with or ends on: `Start`, `File`, `Tool`, `Command`, the Kodierfreigabe, the Zielprofil. */
export type LogFact = { key: FactKey; value: string };

export type FactKey =
  | "Start" | "File" | "Tool" | "Command" | "Encode allowance" | "Target profile"
  | "Delete original" | "Force" | "State" | "Encode need" | "Planned change";

const FACT_KEYS: readonly FactKey[] = [
  "Start", "File", "Tool", "Command", "Encode allowance", "Target profile",
  "Delete original", "Force", "State", "Encode need", "Planned change",
];

export type RuleState = "ok" | "warning" | "blocker" | "other";

export type LogRow =
  | { kind: "phase"; name: string; started?: string; took?: string; bytes?: number }
  | { kind: "plan"; stream: string; action: string }
  | { kind: "stream"; index: string; type: string; codec: string; parts: string[] }
  | { kind: "rule"; name: string; track?: string; state: RuleState; detail: string }
  | { kind: "check"; passed: boolean; blockers: number; warnings: number }
  | { kind: "outcome"; value: string }
  | { kind: "original"; kept: boolean; why: string }
  | { kind: "error"; text: string }
  | { kind: "end"; exit?: string; duration?: string }
  | { kind: "omitted"; bytes: number }
  | { kind: "note"; text: string; bullet: boolean; time?: string };

export type LogDoc = {
  facts: LogFact[];
  rows: LogRow[];
  /** The last percentage the tool reported, 0 to 100. */
  progress: number | undefined;
};

const STAMP = /^\[(\d{2}:\d{2}:\d{2})\]\s?/;
const PROGRESS = /^ATVPREP-PROGRESS\s+(\d{1,3})\s*$/;
const OMITTED = /^…\s*(\d+)\s+bytes omitted\s*…$/;
const PHASE_STEP = /^Phase\s+(\S+?):\s+started\s+(\S+?),\s+took\s+(.+)$/;
const RULE = /^Rule\s+(\S+?)(?:\s+\(track\s+(\d+)\))?:\s+(.*)$/;
const STREAM = /^Stream\s+(\d+):\s+(\S+)\s+(\S+)((?:,\s*.+)?)$/;
const BULLET = /^\s*[•·*-]\s+(.*)$/;

const clamp = (value: number) => Math.max(0, Math.min(100, value));

function ruleState(rest: string): { state: RuleState; detail: string } {
  const [head = "", ...more] = rest.split(/:\s+/);
  const detail = more.join(": ");
  const word = head.trim().toLowerCase();
  if (word === "ok") return { state: "ok", detail };
  if (word === "warning") return { state: "warning", detail };
  if (word === "blocker" || word === "error" || word === "failed") return { state: "blocker", detail };
  return { state: "other", detail: rest };
}

/** One JSON line of the phase timing: `{"phase":"verify","seconds":2.5,"bytes":123}`. */
function phaseRecord(line: string): { name: string; seconds?: number; bytes?: number } | undefined {
  if (!line.startsWith("{") || !line.endsWith("}")) return undefined;
  try {
    const record: unknown = JSON.parse(line);
    if (typeof record !== "object" || record === null) return undefined;
    const fields = record as Record<string, unknown>;
    if (typeof fields.phase !== "string") return undefined;
    const seconds = typeof fields.seconds === "number" ? fields.seconds : undefined;
    const bytes = typeof fields.bytes === "number" ? fields.bytes : undefined;
    return { name: fields.phase, ...(seconds !== undefined ? { seconds } : {}), ...(bytes !== undefined ? { bytes } : {}) };
  } catch {
    return undefined;
  }
}

function seconds(value: number): string {
  if (value < 60) return `${value.toFixed(2)} s`;
  const whole = Math.round(value);
  const m = Math.floor(whole / 60);
  const s = whole % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function parseLog(text: string): LogDoc {
  const doc: LogDoc = { facts: [], rows: [], progress: undefined };
  let end: Extract<LogRow, { kind: "end" }> | undefined;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\r$/, "");
    if (!line.trim()) continue;

    const stamp = STAMP.exec(line);
    const time = stamp?.[1];
    const body = (stamp ? line.slice(stamp[0].length) : line).trimEnd();
    const trimmed = body.trim();
    if (!trimmed) continue;

    const progress = PROGRESS.exec(trimmed);
    if (progress) {
      doc.progress = clamp(Number(progress[1]));
      continue;
    }
    const omitted = OMITTED.exec(trimmed);
    if (omitted) {
      doc.rows.push({ kind: "omitted", bytes: Number(omitted[1]) });
      continue;
    }

    const record = phaseRecord(trimmed);
    if (record) {
      const row: LogRow = { kind: "phase", name: record.name, ...(record.seconds !== undefined ? { took: seconds(record.seconds) } : {}), ...(record.bytes !== undefined ? { bytes: record.bytes } : {}) };
      doc.rows.push(row);
      continue;
    }
    const phase = PHASE_STEP.exec(trimmed);
    if (phase) {
      const [, name = "", started = "", took = ""] = phase;
      // The timing record just before it described the same phase: one row, with both.
      const last = doc.rows[doc.rows.length - 1];
      if (last?.kind === "phase" && last.name === name && last.started === undefined) {
        last.started = started;
        last.took = took;
      } else {
        doc.rows.push({ kind: "phase", name, started, took });
      }
      continue;
    }

    if (trimmed.startsWith("ATVPREP-PLAN ") || /^Plan\s/.test(trimmed)) {
      const plan = trimmed.replace(/^(ATVPREP-PLAN|Plan)\s+/, "");
      const colon = plan.indexOf(":");
      doc.rows.push(colon > 0
        ? { kind: "plan", stream: plan.slice(0, colon).trim(), action: plan.slice(colon + 1).trim() }
        : { kind: "plan", stream: "", action: plan });
      continue;
    }
    const verify = /^(?:ATVPREP-VERIFY|Check after prepare:)\s+(passed|failed),\s*(\d+)\s+blockers?,\s*(\d+)\s+warnings?/i.exec(trimmed);
    if (verify) {
      doc.rows.push({ kind: "check", passed: verify[1]!.toLowerCase() === "passed", blockers: Number(verify[2]), warnings: Number(verify[3]) });
      continue;
    }
    const outcome = /^ATVPREP-OUTCOME\s+(.+)$/.exec(trimmed);
    if (outcome) {
      doc.rows.push({ kind: "outcome", value: outcome[1]!.trim() });
      continue;
    }
    if (trimmed.startsWith("ATVPREP-STAGE ") || trimmed.startsWith("ATVPREP-ENCODE ")) continue; // machine lines the steps already say
    if (trimmed === "ATVPREP-SOURCE removed") continue; // "Original: removed" follows
    const original = /^Original:\s+(removed|kept)(?:\s+\((.*)\))?\s*$/.exec(trimmed);
    if (original) {
      doc.rows.push({ kind: "original", kept: original[1] === "kept", why: original[2] ?? "" });
      continue;
    }
    if (trimmed.startsWith("ATVPREP-SOURCE")) continue;
    const error = /^ATVPREP-ERROR\s+(.+)$/.exec(trimmed) ?? /^(?:Failed|Interrupted|Cancelled)\b.*$/.exec(trimmed);
    if (error) {
      doc.rows.push({ kind: "error", text: error[1] ?? trimmed });
      continue;
    }
    const exit = /^Exit code:\s*(.+)$/.exec(trimmed);
    const duration = /^Duration:\s*(.+)$/.exec(trimmed);
    if (exit || duration) {
      end ??= { kind: "end" };
      if (exit) end.exit = exit[1]!.trim();
      if (duration) end.duration = duration[1]!.trim();
      if (!doc.rows.includes(end)) doc.rows.push(end);
      continue;
    }
    const rule = RULE.exec(trimmed);
    if (rule) {
      const { state, detail } = ruleState(rule[3]!);
      doc.rows.push({ kind: "rule", name: rule[1]!, ...(rule[2] ? { track: rule[2] } : {}), state, detail });
      continue;
    }
    const stream = STREAM.exec(trimmed);
    if (stream) {
      const parts = (stream[4] ?? "").split(",").map((part) => part.trim()).filter(Boolean);
      doc.rows.push({ kind: "stream", index: stream[1]!, type: stream[2]!, codec: stream[3]!, parts });
      continue;
    }

    const fact = FACT_KEYS.find((key) => trimmed.startsWith(`${key}: `));
    if (fact) {
      doc.facts.push({ key: fact, value: trimmed.slice(fact.length + 2) });
      continue;
    }

    const bullet = BULLET.exec(body);
    doc.rows.push({ kind: "note", text: bullet ? bullet[1]!.trim() : trimmed, bullet: !!bullet, ...(time ? { time } : {}) });
  }
  return doc;
}

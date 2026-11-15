import { tn, t } from "../../lib/i18n";
import type { PlannedChange } from "../../lib/encode";
import { formatPlanStep, isSampleRate, isText, type PlanStep } from "./planSteps";
import { parseReason, type ParsedReason } from "./reasons";

/**
 * What `atvprep prepare` will do about each verify reason, in the words of the server's Änderungsart.
 * Derived from the rules, not simulated: the size-neutral postfix steps and the Apple tag patch change
 * the file without re-encoding ("patch"), layout problems need a remux, a rule that adds an audio track
 * is "audio", the video re-encode is "reencode", the encodings the planner names are "audio" from its
 * data, a step of a kind not known yet is "unknown", and what nothing here can predict is "none".
 */
export type Change = { kind: PlannedChange; rule: string | null; text: string };

const TAG_FINDINGS: ReadonlyArray<[RegExp, (n: number) => string]> = [
  [/^(\d+) tracks? lacks? an elng tag$/, (n) => tn("Write {count} elng language tag", "Write {count} elng language tags", n)],
  [/^(\d+) surround tracks? (?:has|have) no stereo fallback$/, (n) => tn("Link {count} surround track to its stereo twin (fall)", "Link {count} surround tracks to their stereo twins (fall)", n)],
  [/^(\d+) audio tracks? (?:does|do) not follow /, (n) => tn("Link {count} audio track to the forced subtitle (folw)", "Link {count} audio tracks to the forced subtitle (folw)", n)],
  [/^(\d+) subtitle tracks? (?:is|are) not linked /, (n) => tn("Link {count} subtitle track to its forced variant (forc)", "Link {count} subtitle tracks to their forced variants (forc)", n)],
  [/^(\d+) forced subtitle tracks? (?:lacks?|lack) the forced display flags$/, (n) => tn("Set the forced display flags on {count} subtitle track", "Set the forced display flags on {count} subtitle tracks", n)],
];

function tagChange(message: string): Change {
  if (message === "track references differ from the expected layout") {
    return { kind: "patch", rule: "apple-tags", text: t("Rewrite the track references to the expected layout") };
  }
  for (const [pattern, describe] of TAG_FINDINGS) {
    const match = pattern.exec(message);
    if (match) return { kind: "patch", rule: "apple-tags", text: describe(Number(match[1])) };
  }
  return { kind: "patch", rule: "apple-tags", text: message };
}

/**
 * An encoding the planner names. Its codecs and rates are data, so the text is formatted, never read
 * back: a step known only as text (an older result) stays as it came, and a kind this build does not
 * know is shown by name.
 */
function planChange(step: PlanStep): Change {
  return { kind: isSampleRate(step) || isText(step) ? "audio" : "unknown", rule: isSampleRate(step) || isText(step) ? "audio-sample-rate" : null, text: formatPlanStep(step) };
}

/** A rule that threw reports `rule crashed: ...`; that is a tool problem, not a defect of the file. */
const CRASHED_PREFIX = "rule crashed:";

function changeFor(reason: ParsedReason, planned: boolean): Change | null {
  if (!reason.severity || !reason.rule) return null;
  const rule = reason.rule;
  if (reason.message.startsWith(CRASHED_PREFIX)) {
    return { kind: "none", rule, text: t("The check itself failed ({message}); check the file again", { message: reason.message }) };
  }
  switch (rule) {
    // verify and prepare judge Atmos by the same signal (ffprobe), so "missing 'dby1'" means real
    // Atmos that only a remux can brand; a dec3 claim without Atmos is cleared in place.
    case "ftyp-dby1-requires-atmos":
      if (reason.message.includes("missing 'dby1'")) return { kind: "remux", rule, text: t("Rewrite the file so its header carries the dby1 brand") };
      if (reason.message.includes("dec3 box claims Dolby Atmos")) return { kind: "patch", rule, text: t("Clear the Atmos flag the audio track claims without carrying Atmos") };
      return { kind: "patch", rule, text: t("Remove the dby1 brand from the file header") };
    case "single-enabled-per-type": return { kind: "patch", rule, text: t("Enable one video track, the stereo audio and only its forced subtitle") };
    case "default-track-selection": return { kind: "patch", rule, text: t("Enable the tracks Apple enables: stereo audio and its forced subtitle") };
    case "subtitle-handler-type": return { kind: "patch", rule, text: t("Mark subtitle tracks as subtitles (sbtl)") };
    case "apple-tags": return tagChange(reason.message);
    case "stereo-twin": return { kind: "audio", rule, text: t("Add an AAC stereo track for the surround-only language") };
    case "duplicate-audio": return { kind: "remux", rule, text: t("Drop the duplicate audio track") };
    case "video-reencode": return { kind: "reencode", rule, text: t("Re-encode the video to HEVC (lossy, needs confirmation)") };
    case "fast-start": return { kind: "remux", rule, text: t("Rewrite the file with the index at the front (faststart)") };
    case "no-fragments": return { kind: "remux", rule, text: t("Rewrite the fragmented file as a regular MP4") };
    // What Prepare does about it, the planner says in a step of its own; without one nothing here guesses.
    case "audio-sample-rate":
      return planned ? null : { kind: "none", rule, text: t("{message}. Check the file again to see what Prepare plans.", { message: reason.message }) };
    case "avc-level-atv-limit":
    case "hevc-level-atv-limit":
      return { kind: "none", rule, text: t("{message}. Only a re-encode changes this.", { message: reason.message }) };
    default:
      return { kind: "none", rule, text: t("{message} (what Prepare does about this rule is unknown)", { message: reason.message }) };
  }
}

/** What a fix does for the verify reasons of a file and the steps the planner named for it. */
export function expectedChanges(reasons: ParsedReason[], steps: ReadonlyArray<PlanStep> = []): Change[] {
  return [
    ...reasons.flatMap((reason) => {
      const change = changeFor(reason, steps.length > 0);
      return change ? [change] : [];
    }),
    ...steps.map(planChange),
  ];
}

const EMBED_STEP = /^\[embed\] subtitles: (\d+)$/;

/**
 * The planned steps a Vorbereiten item was queued with, in the words of the preview: the planner's own
 * steps, the verify reasons it resolves and the subtitle files it merges (`[embed] subtitles: n`). What
 * it leaves unchanged is not a step; a text step that is none of these stays as it came.
 */
export function plannedStepTexts(steps: ReadonlyArray<PlanStep>): string[] {
  return steps.flatMap((step) => {
    if (!isText(step)) return [formatPlanStep(step)];
    const embed = EMBED_STEP.exec(step.text);
    if (embed) {
      const count = Number(embed[1]);
      return [tn("Merge {count} external subtitle file into the MP4", "Merge {count} external subtitle files into the MP4", count)];
    }
    const reason = parseReason(step.text);
    if (!reason.severity) return [step.text];
    return expectedChanges([reason]).filter((change) => change.kind !== "none").map((change) => change.text);
  });
}

/** Whether a blocked file can be repaired by a fix, needs hand work, or was never really checked. */
export function blockerVerdict(reasons: ParsedReason[]): "fixable" | "manual" | "check-failed" {
  const blockers = reasons.filter((reason) => reason.severity === "blocker");
  if (blockers.some((reason) => reason.message.startsWith(CRASHED_PREFIX))) return "check-failed";
  const fixable = blockers.length > 0 && blockers.every((reason) => (changeFor(reason, false)?.kind ?? "none") !== "none");
  return fixable ? "fixable" : "manual";
}

export function summarizeChanges(changes: Change[]): { resolved: number; remaining: number; remux: boolean } {
  return {
    resolved: changes.filter((change) => change.kind !== "none").length,
    remaining: changes.filter((change) => change.kind === "none").length,
    remux: changes.some((change) => change.kind === "remux"),
  };
}

/**
 * Whether queueing a fix for this file can change anything. Without a readable violation the
 * server's own request for a fix is trusted; with only unfixable ones it would be a pointless job.
 */
export function fixesAnything(file: { compliance_reasons?: string[] | undefined; planned_steps?: ReadonlyArray<PlanStep> | undefined; pending_sidecars?: number | undefined }): boolean {
  if ((file.pending_sidecars ?? 0) > 0) return true;
  const changes = expectedChanges((file.compliance_reasons ?? []).map(parseReason), file.planned_steps ?? []);
  return changes.length === 0 || changes.some((change) => change.kind !== "none");
}

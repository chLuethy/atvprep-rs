import { formatNumber, tn, t } from "../../lib/i18n";
import { parseReason, type ParsedReason } from "./reasons";

/**
 * What `atvprep prepare` will do about each verify reason. Derived from the rules, not simulated:
 * the size-neutral postfix steps and the Apple tag patch change the file without re-encoding
 * ("patch"), layout problems need a remux, findings that need encoding are "encode" (whether that means
 * audio or video is the server's Kodierbedarf, not a guess from here), an audio track at a sample rate the
 * Apple TV does not take is encoded at 48 kHz ("encode"), and level problems nothing here can predict
 * ("none").
 */
export type ChangeKind = "patch" | "remux" | "encode" | "none";
export type Change = { kind: ChangeKind; rule: string | null; text: string };

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

/** The codec as mp4info names it in `audio-sample-rate`, and what Prepare encodes it to at 48 kHz. */
const RESAMPLED: Readonly<Record<string, [string, string]>> = {
  "ac-3": ["AC-3", "EAC3"], ac3: ["AC-3", "EAC3"], "ec-3": ["E-AC-3", "EAC3"], ec3: ["E-AC-3", "EAC3"], mp4a: ["AAC", "AAC"],
};
const SAMPLE_RATE = /^(\S+) track id=\S+ sample rate (\d+)Hz /;
const kHz = (hz: number) => formatNumber(hz / 1000, hz % 1000 === 0 ? 0 : 1);

/** Prepare encodes an AC-3/E-AC-3 track that is not at 48 kHz to EAC3, an odd-rate AAC to AAC, both at 48 kHz. */
function sampleRateChange(message: string): Change | null {
  const match = SAMPLE_RATE.exec(message);
  const codecs = match ? RESAMPLED[match[1] ?? ""] : undefined;
  if (!match || !codecs) return null;
  return { kind: "encode", rule: "audio-sample-rate", text: `${codecs[0]} ${kHz(Number(match[2]))} kHz → ${codecs[1]} ${kHz(48000)} kHz` };
}

/** A rule that threw reports `rule crashed: ...`; that is a tool problem, not a defect of the file. */
const CRASHED_PREFIX = "rule crashed:";

function changeFor(reason: ParsedReason): Change | null {
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
    case "stereo-twin": return { kind: "encode", rule, text: t("Add an AAC stereo track for the surround-only language") };
    case "duplicate-audio": return { kind: "remux", rule, text: t("Drop the duplicate audio track") };
    case "video-reencode": return { kind: "encode", rule, text: t("Re-encode the video to HEVC (lossy, needs confirmation)") };
    case "fast-start": return { kind: "remux", rule, text: t("Rewrite the file with the index at the front (faststart)") };
    case "no-fragments": return { kind: "remux", rule, text: t("Rewrite the fragmented file as a regular MP4") };
    case "audio-sample-rate":
      return sampleRateChange(reason.message) ?? { kind: "none", rule, text: t("{message}. Only a re-encode changes this.", { message: reason.message }) };
    case "avc-level-atv-limit":
    case "hevc-level-atv-limit":
      return { kind: "none", rule, text: t("{message}. Only a re-encode changes this.", { message: reason.message }) };
    default:
      return { kind: "none", rule, text: t("{message} (what Prepare does about this rule is unknown)", { message: reason.message }) };
  }
}

export function expectedChanges(reasons: ParsedReason[]): Change[] {
  return reasons.flatMap((reason) => {
    const change = changeFor(reason);
    return change ? [change] : [];
  });
}

const EMBED_STEP = /^\[embed\] subtitles: (\d+)$/;

/**
 * The planned steps a Vorbereiten item was queued with, in the words of the preview: the verify reasons
 * it resolves and the subtitle files it merges (`[embed] subtitles: n`). What it leaves unchanged is
 * not a step.
 */
export function plannedStepTexts(steps: ReadonlyArray<string>): string[] {
  return steps.flatMap((step) => {
    const embed = EMBED_STEP.exec(step);
    if (embed) {
      const count = Number(embed[1]);
      return [tn("Merge {count} external subtitle file into the MP4", "Merge {count} external subtitle files into the MP4", count)];
    }
    return expectedChanges([parseReason(step)]).filter((change) => change.kind !== "none").map((change) => change.text);
  });
}

/** Whether a blocked file can be repaired by a fix, needs hand work, or was never really checked. */
export function blockerVerdict(reasons: ParsedReason[]): "fixable" | "manual" | "check-failed" {
  const blockers = reasons.filter((reason) => reason.severity === "blocker");
  if (blockers.some((reason) => reason.message.startsWith(CRASHED_PREFIX))) return "check-failed";
  const fixable = blockers.length > 0 && blockers.every((reason) => (changeFor(reason)?.kind ?? "none") !== "none");
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
export function fixesAnything(file: { compliance_reasons?: string[] | undefined; pending_sidecars?: number | undefined }): boolean {
  if ((file.pending_sidecars ?? 0) > 0) return true;
  const changes = expectedChanges((file.compliance_reasons ?? []).map(parseReason));
  return changes.length === 0 || changes.some((change) => change.kind !== "none");
}

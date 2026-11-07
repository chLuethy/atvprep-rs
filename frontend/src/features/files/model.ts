import type { ChipKind } from "../../ui/StatusChip";
import type { IconName } from "../../ui/icons";
import { formatNumber, N, t } from "../../lib/i18n";
import { blockerVerdict } from "./fixPlan";
import { parseReason } from "./reasons";

export { parseReason, type ParsedReason } from "./reasons";

/** Pure data logic for the library: verdicts, reasons, tracks, formatting. No DOM, no network. */

export type Verdict = { kind: ChipKind; icon: IconName; label: string };

const humanize = (value: string) => value.replaceAll("_", " ").replace(/^\w/, (character) => character.toUpperCase());

/** Only a verify report can make a file Ready; an unverified file is never shown as fine. */
export function verdictOf(file: { compliance_state: string; discovery_state?: string; compliance_reasons?: string[] | undefined; pending_sidecars?: number | undefined }): Verdict {
  if (file.discovery_state === "missing") return { kind: "bad", icon: "x", label: t("Missing") };
  switch (file.compliance_state) {
    // Clean is only Ready while nothing is left to embed; the stored state stays clean.
    case "clean":
      if ((file.pending_sidecars ?? 0) > 0) return { kind: "warn", icon: "dot", label: t("Embed open") };
      return { kind: "ok", icon: "check", label: t("Ready") };
    case "warning": return { kind: "warn", icon: "warn", label: t("Warning") };
    case "blocker": {
      const outcome = blockerVerdict((file.compliance_reasons ?? []).map(parseReason));
      const label = outcome === "check-failed" ? t("Check failed") : outcome === "fixable" ? t("Blocker, fixable") : t("Blocker");
      return { kind: "bad", icon: "x", label };
    }
    case "probe_failed": return { kind: "bad", icon: "x", label: t("Inspection failed") };
    case "unknown": return { kind: "neutral", icon: "dot", label: t("Not verified") };
    default: return { kind: "neutral", icon: "dot", label: humanize(file.compliance_state) };
  }
}

export function isFixable(file: { discovery_state: string; compliance_state: string; desired_action: string; pending_sidecars?: number | undefined }): boolean {
  if (file.discovery_state !== "present") return false;
  return file.compliance_state === "warning" || file.compliance_state === "blocker" || file.desired_action === "fix" || (file.pending_sidecars ?? 0) > 0;
}

/** What a fix does with one external subtitle file, in words a person can act on. */
export function describeSidecar(state: string | undefined, language: string | undefined): string {
  const name = languageName(language);
  switch (state) {
    case "pending": return t("{name}: Prepare embeds it", { name });
    case "duplicate": return t("{name}: not used, another {name} subtitle file is embedded instead", { name });
    case "embedded": return t("{name}: not used, the file already has this subtitle", { name });
    case "ignored_language": return t("{name}: not used, only German, English and the audio languages are kept", { name });
    case "unknown_language": return t("Language not recognised from the name. Name it like Film.de.srt");
    case "unsupported": return t("Not used: only .srt files are embedded");
    case "unchecked": return t("{name}: not known yet whether Prepare embeds it. Re-inspect the file first.", { name });
    default: return "";
  }
}

export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(Math.max(bytes, 1)) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${formatNumber(value, value >= 10 || index === 0 ? 0 : 1)} ${units[index]}`;
}

export function formatBitrate(bitsPerSecond: number | undefined): string {
  if (bitsPerSecond === undefined || !Number.isFinite(bitsPerSecond) || bitsPerSecond <= 0) return "—";
  return `${formatNumber(bitsPerSecond / 1_000_000, 1)} Mbps`;
}

export function formatDuration(seconds: number | undefined): string {
  if (seconds === undefined || !Number.isFinite(seconds) || seconds <= 0) return "—";
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))} min`;
  const minutes = Math.floor((seconds % 3600) / 60);
  return `${Math.floor(seconds / 3600)}:${String(minutes).padStart(2, "0")} h`;
}

/** Minutes to rewrite `bytes` at `megabytesPerSecond` (default 120 MB/s); at least one for any real file. */
export function estimateMinutes(bytes: number, megabytesPerSecond = 120): number {
  if (!Number.isFinite(bytes) || bytes <= 0) return 0;
  return Math.max(1, Math.round(bytes / (megabytesPerSecond * 1024 ** 2) / 60));
}

export function baseName(displayPath: string): { title: string; folder: string } {
  if (!displayPath) return { title: "", folder: "" };
  const parts = displayPath.split("/");
  const file = parts.pop() ?? "";
  const dot = file.lastIndexOf(".");
  return { title: dot > 0 ? file.slice(0, dot) : file, folder: parts.join("/") };
}

const LANGUAGES: Record<string, string> = {
  deu: N("German"), ger: N("German"), eng: N("English"), fra: N("French"), fre: N("French"), spa: N("Spanish"), ita: N("Italian"),
  nld: N("Dutch"), dut: N("Dutch"), jpn: N("Japanese"), kor: N("Korean"), zho: N("Chinese"), chi: N("Chinese"), rus: N("Russian"),
  por: N("Portuguese"), tur: N("Turkish"), pol: N("Polish"), swe: N("Swedish"), dan: N("Danish"), nor: N("Norwegian"), fin: N("Finnish"),
};

export function languageName(code: string | undefined): string {
  if (!code || code === "und") return t("Unknown");
  const known = LANGUAGES[code.toLowerCase()];
  return known ? t(known) : code;
}

export type TrackKind = "video" | "audio" | "subtitle" | "other";

export type Track = {
  kind: TrackKind;
  index: number;
  /** V0, A0, A1, S0 … numbered per kind; empty for streams that are not media tracks. */
  label: string;
  codec: string;
  /** Resolution for video, channel layout for audio. */
  detail: string;
  language: string | undefined;
  isDefault: boolean;
  isForced: boolean;
};

export type InspectionSummary = {
  container: string | undefined;
  durationSeconds: number | undefined;
  bitrate: number | undefined;
  tracks: Track[];
};

const asRecord = (value: unknown): Record<string, unknown> | undefined =>
  typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
const positiveNumber = (value: unknown): number | undefined => {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number.parseFloat(value) : Number.NaN;
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
};
const text = (value: unknown): string | undefined => (typeof value === "string" && value !== "" ? value : undefined);

function channelLabel(channels: number | undefined, layout: string | undefined): string {
  if (channels === 1) return "Mono";
  if (channels === 2) return "Stereo";
  if (channels === 6) return "5.1";
  if (channels === 8) return "7.1";
  if (channels !== undefined) return `${channels} ch`;
  return layout ?? "";
}

/** Reads the parts of an ffprobe result the library shows. Anything malformed is skipped, never thrown on. */
export function summarizeInspection(result: Record<string, unknown>): InspectionSummary {
  const format = asRecord(result.format);
  const counters: Record<TrackKind, number> = { video: 0, audio: 0, subtitle: 0, other: 0 };
  const prefixes: Record<TrackKind, string> = { video: "V", audio: "A", subtitle: "S", other: "" };
  const tracks: Track[] = [];
  const streams = Array.isArray(result.streams) ? result.streams : [];
  streams.forEach((raw, position) => {
    const stream = asRecord(raw);
    if (!stream) return;
    const type = text(stream.codec_type);
    const kind: TrackKind = type === "video" || type === "audio" || type === "subtitle" ? type : "other";
    const disposition = asRecord(stream.disposition);
    const tags = asRecord(stream.tags);
    const width = positiveNumber(stream.width);
    const height = positiveNumber(stream.height);
    const detail = kind === "video"
      ? (width && height ? `${width}×${height}` : "")
      : kind === "audio"
        ? channelLabel(positiveNumber(stream.channels), text(stream.channel_layout))
        : "";
    const number = counters[kind]++;
    tracks.push({
      kind,
      index: typeof stream.index === "number" ? stream.index : position,
      label: kind === "other" ? "" : `${prefixes[kind]}${number}`,
      codec: text(stream.codec_name) ?? "unknown",
      detail,
      language: text(tags?.language),
      isDefault: disposition?.default === 1,
      isForced: disposition?.forced === 1,
    });
  });
  return {
    container: text(format?.format_name),
    durationSeconds: positiveNumber(format?.duration),
    bitrate: positiveNumber(format?.bit_rate),
    tracks,
  };
}

/** The reason the manager records after a successful fix, which describes the old bytes no longer. */
export function wasJustFixed(file: { compliance_state: string; compliance_reasons?: string[] | undefined }): boolean {
  return file.compliance_state === "unknown" && (file.compliance_reasons ?? [])[0]?.startsWith("Fixed by atvprep") === true;
}

import { t } from "../../lib/i18n";
import { formatBitrate } from "./model";

/**
 * Bitrate against the Max Streaming Bitrate the user set in Wave. Only bitrate is judged:
 * the container and codecs matter too, so "within the limit" never promises direct play.
 */
export const LIMIT_OPTIONS_MBPS = [10, 20, 40, 80, 120] as const;
export const DEFAULT_LIMIT_MBPS = 80;

const KEY = "atvprep-wave-limit-mbps";

export function readWaveLimit(): number {
  try {
    const stored = Number(window.localStorage.getItem(KEY));
    return (LIMIT_OPTIONS_MBPS as readonly number[]).includes(stored) ? stored : DEFAULT_LIMIT_MBPS;
  } catch {
    return DEFAULT_LIMIT_MBPS;
  }
}

export function writeWaveLimit(limit: number): void {
  try {
    window.localStorage.setItem(KEY, String(limit));
  } catch {
    // Blocked storage only costs the remembered choice.
  }
}

export type WaveVerdict = { state: "direct" | "transcode" | "unknown"; headline: string; detail: string };

export function waveVerdict(bitrate: number | undefined, limitMbps: number): WaveVerdict {
  if (bitrate === undefined || !Number.isFinite(bitrate) || bitrate <= 0) {
    return { state: "unknown", headline: t("Bitrate unknown"), detail: t("Inspect the file to compare its bitrate with your Wave limit.") };
  }
  const shown = formatBitrate(bitrate);
  if (bitrate > limitMbps * 1_000_000) {
    return {
      state: "transcode",
      headline: t("Wave transcodes this file"),
      detail: t("{shown} is over your {limit} Mbps limit. A fix does not change this: raise the limit in Wave or re-encode the file.", { shown, limit: limitMbps }),
    };
  }
  return {
    state: "direct",
    headline: t("Within your limit"),
    detail: t("{shown} is within your {limit} Mbps limit. Bitrate alone does not force a transcode; the container and codecs still decide.", { shown, limit: limitMbps }),
  };
}

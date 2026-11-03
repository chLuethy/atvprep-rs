import { t } from "../../lib/i18n";
import type { Track } from "./model";

const MP4_SUFFIXES = new Set([".mp4", ".m4v", ".mov"]);

/** MP4-like files are verified and fixed in place; everything else has to be converted first. */
export const isMp4Like = (suffix: string): boolean => MP4_SUFFIXES.has(suffix.toLowerCase());

const VIDEO_COPY = new Set(["h264", "hevc"]);
const AUDIO_COPY = new Set(["aac", "ac3", "eac3"]);

export type ConvertPlan = {
  /** The picture is re-encoded to HEVC. That is the slow case: hours rather than minutes. */
  videoEncode: boolean;
  audioEncode: boolean;
  lines: string[];
};

/**
 * What `atvprep convert` will most likely do, read from the codecs ffprobe found. H.264 up to 1080p
 * and HEVC are copied into the MP4, and AAC, AC-3 and E-AC-3 are copied too; any other video codec
 * (or H.264 above 1080p) is encoded to HEVC, and other audio to E-AC-3. HDR and bit depth are not
 * visible here, so atvprep can still decide to encode a file this predicts as a copy.
 */
export function convertPlan(tracks: ReadonlyArray<Track>): ConvertPlan {
  const video = tracks.find((track) => track.kind === "video");
  const audio = tracks.filter((track) => track.kind === "audio");
  const height = Number(/×(\d+)$/.exec(video?.detail ?? "")?.[1] ?? 0);
  const videoEncode = video !== undefined && (!VIDEO_COPY.has(video.codec) || (video.codec === "h264" && height > 1080));
  const audioEncode = audio.some((track) => !AUDIO_COPY.has(track.codec));
  const lines: string[] = [];
  if (video) {
    const codec = video.codec.toUpperCase();
    lines.push(videoEncode
      ? t("Video {codec} is re-encoded to HEVC", { codec: `${codec}${video.detail ? ` ${video.detail}` : ""}` })
      : t("Video {codec} is copied into the MP4", { codec }));
  }
  if (audio.length > 0) lines.push(audioEncode ? t("Audio that Apple TV cannot play is re-encoded to E-AC-3") : t("Audio is copied into the MP4"));
  return { videoEncode, audioEncode, lines };
}

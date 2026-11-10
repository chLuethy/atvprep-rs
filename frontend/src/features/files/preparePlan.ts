import { N, t } from "../../lib/i18n";
import { isMp4Like } from "./container";
import { fixesAnything } from "./fixPlan";
import { isFixable } from "./model";

/** The Kodierbedarf the server stored for a file; a value it has not sent yet counts as unknown. */
export type EncodeNeed = "none" | "audio" | "video" | "unknown";
/** The Änderungsart the server stored for a file. */
export type PlannedChange = "none" | "patch" | "remux" | "audio" | "reencode" | "unknown";

const NEED_LABEL: Record<EncodeNeed, string> = { none: N("No encoding"), audio: N("Audio"), video: N("Video"), unknown: N("Unknown") };
const CHANGE_LABEL: Record<PlannedChange, string> = { none: N("Unchanged"), patch: N("Patch"), remux: N("Remux"), audio: N("Audio encoding"), reencode: N("Re-encode"), unknown: N("Unknown") };

export const needLabel = (need: EncodeNeed | undefined): string => t(NEED_LABEL[need ?? "unknown"]);
export const changeLabel = (change: PlannedChange | undefined): string => t(CHANGE_LABEL[change ?? "unknown"]);

export type PrepareConfirm = {
  /** What Prepare is about to do to this file, in the words the confirmation shows. */
  lines: string[];
  /** The video has to be re-encoded, which only the level „Encode audio and video“ allows (ADR 0008). */
  reencode: boolean;
};

/** What the confirmation for one file says: the server's Kodierbedarf, in words, and what a level does with it. */
export function prepareConfirm(file: { suffix: string; encode_need?: EncodeNeed | undefined }): PrepareConfirm {
  const mp4 = isMp4Like(file.suffix);
  if (file.encode_need === "video") {
    return {
      lines: [t("The video has to be re-encoded. That only runs with “Encode audio and video”; otherwise the file stays as it is and the job says so.")],
      reencode: true,
    };
  }
  if (file.encode_need === "audio") {
    return { lines: [t("Audio has to be encoded. That only runs with “Encode audio” or higher; otherwise the file stays as it is and the job says so.")], reencode: false };
  }
  if (file.encode_need === "unknown") {
    return { lines: [t("The encoding need is not known yet. The run keeps to the level you choose and says so if the file needs more.")], reencode: false };
  }
  if (mp4) {
    return { lines: [t("Prepare rewrites this MP4 in place where needed. Files already at the target are left alone.")], reencode: false };
  }
  return { lines: [t("Prepare writes an MP4 next to this {suffix} file. The original is kept.", { suffix: file.suffix })], reencode: false };
}

/**
 * Whether queueing Prepare for this file can change anything. Another container is always worth
 * preparing unless its converted MP4 is already kept next to it; an MP4 needs a violation Prepare
 * resolves or a subtitle file to merge. A file that needs encoding, or whose need is not known yet,
 * is offered too: the preview sets the allowance and the run enforces it.
 */
export function preparable(file: {
  discovery_state: string;
  suffix: string;
  converted_kept?: boolean | undefined;
  compliance_state: string;
  desired_action: string;
  pending_sidecars?: number | undefined;
  fix_status?: string | null | undefined;
  encode_need?: EncodeNeed | undefined;
}): boolean {
  if (file.discovery_state !== "present" || file.fix_status || file.converted_kept) return false;
  if (!isMp4Like(file.suffix)) return true;
  if (file.encode_need === "audio" || file.encode_need === "video" || file.encode_need === "unknown") return true;
  return isFixable(file) && fixesAnything(file);
}

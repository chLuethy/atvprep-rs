import { t } from "../../lib/i18n";
import { convertPlan, isMp4Like } from "./convertPlan";
import { fixesAnything } from "./fixPlan";
import { isFixable, type Track } from "./model";

export type PrepareConfirm = {
  /** What Prepare is about to do to this file, in the words the confirmation shows. */
  lines: string[];
  /** The video has to be re-encoded, which only the level „Encode audio and video“ allows (ADR 0008). */
  reencode: boolean;
};

/**
 * What the confirmation for one file says. A file that has to be re-encoded says which level that takes.
 * `tracks` are only read for another container, whose re-encode is predicted from its codecs.
 */
export function prepareConfirm(file: { suffix: string; needs_reencode?: boolean | undefined }, tracks: ReadonlyArray<Track>): PrepareConfirm {
  const mp4 = isMp4Like(file.suffix);
  const reencode = file.needs_reencode === true || (!mp4 && convertPlan(tracks).videoEncode);
  if (reencode) {
    return {
      lines: [t("The video has to be re-encoded. That only runs with “Encode audio and video”; otherwise the file stays as it is and the job says so.")],
      reencode: true,
    };
  }
  if (mp4) {
    return { lines: [t("Prepare rewrites this MP4 in place where needed. Files already at the target are left alone.")], reencode: false };
  }
  return { lines: [t("Prepare writes an MP4 next to this {suffix} file. The original is kept.", { suffix: file.suffix })], reencode: false };
}

/**
 * Whether queueing Prepare for this file can change anything. Another container is always worth
 * preparing unless its converted MP4 is already kept next to it; an MP4 needs a violation Prepare
 * resolves or a subtitle file to merge. A video that needs a re-encode is offered too: the preview
 * sets the allowance.
 */
export function preparable(file: {
  discovery_state: string;
  suffix: string;
  converted_kept?: boolean | undefined;
  compliance_state: string;
  desired_action: string;
  pending_sidecars?: number | undefined;
  fix_status?: string | null | undefined;
  needs_reencode?: boolean | undefined;
}): boolean {
  if (file.discovery_state !== "present" || file.fix_status || file.converted_kept) return false;
  if (!isMp4Like(file.suffix)) return true;
  if (file.needs_reencode) return true;
  return isFixable(file) && fixesAnything(file);
}

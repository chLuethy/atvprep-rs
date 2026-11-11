/** The Kodierfreigabe in rising order: how far Prepare may encode (ADR 0008). */
export const ENCODE_LEVELS = ["copy", "audio", "all"] as const;

export type EncodeLevel = (typeof ENCODE_LEVELS)[number];

/** The Kodierbedarf the server stores for a file; „unknown“ until a check against the current Zielprofil. */
export type EncodeNeed = "none" | "audio" | "video" | "unknown";

const REQUIRED: Partial<Record<string, EncodeLevel>> = { none: "copy", audio: "audio", video: "all" };

/** The levels above `encode`, in rising order; empty for the highest or an unknown level. */
export const levelsAbove = (encode: string | null | undefined): EncodeLevel[] => {
  const index = ENCODE_LEVELS.findIndex((level) => level === encode);
  return index < 0 ? [] : ENCODE_LEVELS.slice(index + 1);
};

/** The lowest level a file with this need runs under; none while the need is unknown. */
export const requiredLevel = (need: string | null | undefined): EncodeLevel | undefined => (need ? REQUIRED[need] : undefined);

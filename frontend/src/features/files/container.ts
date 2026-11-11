const MP4_SUFFIXES = new Set([".mp4", ".m4v", ".mov"]);

/** MP4-like files are verified and fixed in place; everything else has to be converted first. */
export const isMp4Like = (suffix: string): boolean => MP4_SUFFIXES.has(suffix.toLowerCase());

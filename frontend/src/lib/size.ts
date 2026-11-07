import { formatNumber } from "./i18n";

const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** A byte count as short text with binary steps: 512 B, 1.5 KB, 2.5 GB. */
export function formatSize(bytes: number): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? String(Math.round(value)) : formatNumber(value, 1)} ${UNITS[unit]}`;
}

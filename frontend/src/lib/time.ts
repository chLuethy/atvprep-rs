import { locale } from "./i18n";

/** A timestamp from the server as local text; a missing or unparseable one reads as `fallback`, never "Invalid Date". */
export function formatDateTime(iso: string | null | undefined, fallback = ""): string {
  if (!iso) return fallback;
  const moment = new Date(iso);
  return Number.isNaN(moment.getTime()) ? fallback : moment.toLocaleString(locale());
}

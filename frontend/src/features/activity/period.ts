import { N } from "../../lib/i18n";

/**
 * The period of the activity filter: when the jobs were queued. A preset is worked out anew each time the list
 * is asked for, so "Today" moves on at midnight in the browser's zone; a custom period is whole local days, both
 * included. The server gets absolute instants: `from` included, `to` left out.
 */
export type PeriodPreset = "today" | "24h" | "7d" | "30d";
/** A preset, or whole days in the browser's zone (`YYYY-MM-DD`, `from` <= `to`, both included). */
export type JobPeriod = { preset: PeriodPreset } | { from: string; to: string };

export const PERIOD_PRESETS: ReadonlyArray<{ token: PeriodPreset; label: string }> = [
  { token: "today", label: N("Today") },
  { token: "24h", label: N("Last 24 hours") },
  { token: "7d", label: N("Last 7 days") },
  { token: "30d", label: N("Last 30 days") },
];

const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** Local midnight of a day written `YYYY-MM-DD`, or null for anything that is no such day. */
export function dayStart(day: string): Date | null {
  const match = DAY.exec(day);
  if (!match) return null;
  const [year, month, date] = [Number(match[1]), Number(match[2]) - 1, Number(match[3])];
  const start = new Date(year, month, date);
  return start.getFullYear() === year && start.getMonth() === month && start.getDate() === date ? start : null;
}

/** A local day as `YYYY-MM-DD`. */
export const dayOf = (date: Date): string =>
  `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;

/** A custom period from two days in any order; null unless both are days. */
export function customPeriod(a: string, b: string): JobPeriod | null {
  if (!dayStart(a) || !dayStart(b)) return null;
  return a <= b ? { from: a, to: b } : { from: b, to: a };
}

/** The instants a period stands for at `now`: `from` included, `to` (if any) left out, as ISO 8601 in UTC. */
export function periodBounds(period: JobPeriod, now: Date): { from: string; to?: string } {
  if ("preset" in period) {
    const from = new Date(now.getTime());
    if (period.preset === "today") from.setHours(0, 0, 0, 0);
    else if (period.preset === "24h") from.setTime(now.getTime() - 24 * 3600 * 1000);
    // By the calendar, so a change of daylight saving keeps the time of day.
    else from.setDate(from.getDate() - (period.preset === "7d" ? 7 : 30));
    return { from: from.toISOString() };
  }
  const from = dayStart(period.from) ?? new Date(now.getTime());
  const to = new Date((dayStart(period.to) ?? from).getTime());
  to.setDate(to.getDate() + 1);
  return { from: from.toISOString(), to: to.toISOString() };
}

/** The period an address asks for: `range=<preset>`, or `from`/`to` days (one given stands for both). */
export function parsePeriod(params: URLSearchParams): JobPeriod | undefined {
  const preset = PERIOD_PRESETS.find((entry) => entry.token === params.get("range"));
  if (preset) return { preset: preset.token };
  const days = [params.get("from"), params.get("to")].filter((day): day is string => !!day && dayStart(day) !== null);
  const [first, second = first] = days;
  return first && second ? customPeriod(first, second) ?? undefined : undefined;
}

/** The address parts of a period, without "?". */
export function periodAddress(period: JobPeriod | undefined): string[] {
  if (!period) return [];
  return "preset" in period ? [`range=${period.preset}`] : [`from=${period.from}`, `to=${period.to}`];
}

/** The server parts of a period at `now`. */
export function periodServer(period: JobPeriod | undefined, now: Date): string[] {
  if (!period) return [];
  const bounds = periodBounds(period, now);
  return [`from=${encodeURIComponent(bounds.from)}`, ...(bounds.to ? [`to=${encodeURIComponent(bounds.to)}`] : [])];
}

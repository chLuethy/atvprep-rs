export type ParsedReason = { severity: "blocker" | "warning" | null; rule: string | null; message: string };

const REASON = /^\[(blocker|warning)\] ([^:]+): ([\s\S]*)$/;

/** Verify reasons arrive as `[severity] rule: message`; anything else stays free text. */
export function parseReason(text: string): ParsedReason {
  const match = REASON.exec(text);
  if (!match) return { severity: null, rule: null, message: text };
  return { severity: match[1] as "blocker" | "warning", rule: match[2] ?? null, message: match[3] ?? "" };
}

const PLAN_STEP = /^\[plan\] ([\s\S]*)$/;

/**
 * A step the planner names in its own words (`[plan] audio stream #1 ac3 44100Hz -> eac3 48000Hz`),
 * stored after the findings of a verify report; null for anything else. It is no finding.
 */
export function planStep(text: string): string | null {
  return PLAN_STEP.exec(text)?.[1] ?? null;
}

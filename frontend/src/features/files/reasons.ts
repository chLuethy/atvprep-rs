export type ParsedReason = { severity: "blocker" | "warning" | null; rule: string | null; message: string };

const REASON = /^\[(blocker|warning)\] ([^:]+): ([\s\S]*)$/;

/** Verify reasons arrive as `[severity] rule: message`; anything else stays free text. */
export function parseReason(text: string): ParsedReason {
  const match = REASON.exec(text);
  if (!match) return { severity: null, rule: null, message: text };
  return { severity: match[1] as "blocker" | "warning", rule: match[2] ?? null, message: match[3] ?? "" };
}

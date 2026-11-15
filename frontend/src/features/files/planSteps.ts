import { formatNumber, t } from "../../lib/i18n";

/**
 * A step the planner names, as the server sends it (ADR 0008): a kind and the data that belongs to it.
 * Nothing here parses text. A kind this build does not know arrives as `{ kind }` with whatever data it
 * has, and is shown by name. `text` is a step known only as words: what older results stored, and the
 * verify findings and the subtitle merge an item lists beside the planner's steps.
 */
export type SampleRateStep = { kind: "sample_rate"; stream: number; from_codec: string; to_codec: string; from_hz: number; to_hz: number };
export type TextStep = { kind: "text"; text: string };
export type PlanStep = SampleRateStep | TextStep | { kind: string; [data: string]: unknown };

export const isSampleRate = (step: PlanStep): step is SampleRateStep => step.kind === "sample_rate";
export const isText = (step: PlanStep): step is TextStep => step.kind === "text";

/** The codec names as the preview writes them. Naming only: codec and rate come from the planner. */
const CODEC_NAMES: Readonly<Record<string, string>> = { ac3: "AC-3", eac3: "EAC3", aac: "AAC" };
const codecName = (codec: string) => CODEC_NAMES[codec] ?? codec.toUpperCase();
const kHz = (hz: number) => formatNumber(hz / 1000, hz % 1000 === 0 ? 0 : 1);

/** One planner step as a line for the operator. */
export function formatPlanStep(step: PlanStep): string {
  if (isSampleRate(step)) return `${codecName(step.from_codec)} ${kHz(step.from_hz)} kHz → ${codecName(step.to_codec)} ${kHz(step.to_hz)} kHz`;
  if (isText(step)) return step.text;
  return t("Planned step: {kind}", { kind: step.kind });
}

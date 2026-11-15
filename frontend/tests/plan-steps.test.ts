import { afterEach, describe, expect, it } from "vitest";
import { setLanguage } from "../src/lib/i18n";
import { formatPlanStep, type PlanStep } from "../src/features/files/planSteps";

afterEach(() => setLanguage("en"));

const rate = (over: Partial<Extract<PlanStep, { kind: "sample_rate" }>> = {}): PlanStep => ({
  kind: "sample_rate",
  stream: 1,
  from_codec: "ac3",
  to_codec: "eac3",
  from_hz: 44100,
  to_hz: 48000,
  ...over,
});

describe("formatPlanStep: the planner's steps, written for the operator", () => {
  it("writes a sample rate change from its data, with the codec names of the preview", () => {
    expect(formatPlanStep(rate())).toBe("AC-3 44.1 kHz → EAC3 48 kHz");
    expect(formatPlanStep(rate({ from_codec: "aac", to_codec: "aac", from_hz: 32000 }))).toBe("AAC 32 kHz → AAC 48 kHz");
    expect(formatPlanStep(rate({ from_codec: "opus" }))).toBe("OPUS 44.1 kHz → EAC3 48 kHz");
  });

  it("shows a step known only as text in its own words", () => {
    expect(formatPlanStep({ kind: "text", text: "audio stream #1 ac3 44100Hz -> eac3 48000Hz" })).toBe("audio stream #1 ac3 44100Hz -> eac3 48000Hz");
  });

  it("names a kind it does not know yet instead of dropping the step", () => {
    expect(formatPlanStep({ kind: "loudness_norm", target: -23 })).toBe("Planned step: loudness_norm");
    setLanguage("de");
    expect(formatPlanStep({ kind: "loudness_norm", target: -23 })).toBe("Geplanter Schritt: loudness_norm");
  });
});

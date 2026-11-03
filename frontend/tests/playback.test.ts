import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_LIMIT_MBPS, LIMIT_OPTIONS_MBPS, readWaveLimit, waveVerdict, writeWaveLimit } from "../src/features/files/playback";

afterEach(() => vi.restoreAllMocks());

describe("waveVerdict", () => {
  it("says Wave will transcode when the bitrate is over the limit, and that a fix will not change it", () => {
    const verdict = waveVerdict(12_129_150, 10);

    expect(verdict.state).toBe("transcode");
    expect(verdict.headline).toBe("Wave transcodes this file");
    expect(verdict.detail).toContain("12.1 Mbps is over your 10 Mbps limit");
    expect(verdict.detail).toMatch(/a fix does not change this/i);
    expect(verdict.detail).toMatch(/raise the limit in wave or re-encode/i);
  });

  it("only claims what bitrate can show when the file is within the limit", () => {
    const verdict = waveVerdict(12_129_150, 20);

    expect(verdict.state).toBe("direct");
    expect(verdict.headline).toBe("Within your limit");
    expect(verdict.detail).toContain("12.1 Mbps is within your 20 Mbps limit");
    expect(verdict.detail).toMatch(/bitrate alone/i);
  });

  it("treats a bitrate exactly at the limit as within it", () => {
    expect(waveVerdict(20_000_000, 20).state).toBe("direct");
    expect(waveVerdict(20_000_001, 20).state).toBe("transcode");
  });

  it("does not guess without a bitrate", () => {
    for (const unknown of [undefined, 0, Number.NaN]) {
      const verdict = waveVerdict(unknown, 10);
      expect(verdict.state).toBe("unknown");
      expect(verdict.detail).toMatch(/inspect/i);
    }
  });
});

describe("the stored Wave limit", () => {
  it("offers Wave's own choices and defaults to its default", () => {
    expect(LIMIT_OPTIONS_MBPS).toEqual([10, 20, 40, 80, 120]);
    expect(DEFAULT_LIMIT_MBPS).toBe(80);
  });

  it("round-trips a valid choice", () => {
    writeWaveLimit(20);
    expect(readWaveLimit()).toBe(20);
  });

  it("falls back to the default for junk, an unknown value or blocked storage", () => {
    vi.spyOn(Storage.prototype, "getItem").mockReturnValueOnce("not a number");
    expect(readWaveLimit()).toBe(DEFAULT_LIMIT_MBPS);
    vi.spyOn(Storage.prototype, "getItem").mockReturnValueOnce("7");
    expect(readWaveLimit()).toBe(DEFAULT_LIMIT_MBPS);
    vi.spyOn(Storage.prototype, "getItem").mockImplementationOnce(() => { throw new Error("blocked"); });
    expect(readWaveLimit()).toBe(DEFAULT_LIMIT_MBPS);
  });

  it("survives a blocked write", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(() => writeWaveLimit(40)).not.toThrow();
  });
});

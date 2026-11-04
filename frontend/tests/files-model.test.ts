import { describe, expect, it } from "vitest";
import {
  baseName,
  estimateMinutes,
  formatBitrate,
  formatDuration,
  formatSize,
  isFixable,
  languageName,
  parseReason,
  summarizeInspection,
  verdictOf,
} from "../src/features/files/model";

describe("parseReason", () => {
  it("splits a verify reason into severity, rule and message", () => {
    expect(parseReason("[blocker] ftyp-dby1-requires-atmos: ftyp lists dby1 but no track is Atmos")).toEqual({
      severity: "blocker",
      rule: "ftyp-dby1-requires-atmos",
      message: "ftyp lists dby1 but no track is Atmos",
    });
    expect(parseReason("[warning] apple-tags: 3 tracks lack elng").severity).toBe("warning");
  });

  it("keeps free text as a message without inventing a severity or rule", () => {
    expect(parseReason("No compliance violations reported by verify.")).toEqual({
      severity: null,
      rule: null,
      message: "No compliance violations reported by verify.",
    });
  });
});

describe("verdictOf", () => {
  it("names every compliance state in words and never calls an unverified file ready", () => {
    expect(verdictOf({ compliance_state: "clean" })).toMatchObject({ kind: "ok", label: "Ready" });
    expect(verdictOf({ compliance_state: "warning" })).toMatchObject({ kind: "warn", label: "Warning" });
    expect(verdictOf({ compliance_state: "blocker" })).toMatchObject({ kind: "bad", label: "Blocker" });
    expect(verdictOf({ compliance_state: "probe_failed" })).toMatchObject({ kind: "bad", label: "Inspection failed" });
    expect(verdictOf({ compliance_state: "unknown" })).toMatchObject({ kind: "neutral", label: "Not verified" });
    expect(verdictOf({ compliance_state: "something_new" })).toMatchObject({ kind: "neutral", label: "Something new" });
  });
});

describe("verdictOf for blockers", () => {
  it("tells fixable, manual and failed checks apart", () => {
    const of = (...compliance_reasons: string[]) => verdictOf({ compliance_state: "blocker", compliance_reasons });
    expect(of("[blocker] no-fragments: fragmented")).toMatchObject({ kind: "bad", icon: "x", label: "Blocker, fixable" });
    expect(of("[blocker] avc-level-atv-limit: level 6.0")).toMatchObject({ kind: "bad", label: "Blocker" });
    expect(of("[blocker] fast-start: rule crashed: boom")).toMatchObject({ kind: "bad", label: "Check failed" });
  });
});

describe("verdictOf for files that left the disk", () => {
  it("reports a missing file as missing whatever its last compliance state was", () => {
    expect(verdictOf({ compliance_state: "clean", discovery_state: "missing" })).toMatchObject({ kind: "bad", label: "Missing" });
    expect(verdictOf({ compliance_state: "clean", discovery_state: "present" })).toMatchObject({ label: "Ready" });
  });
});

describe("isFixable", () => {
  const base = { discovery_state: "present", compliance_state: "blocker", desired_action: "none" };

  it("is true for present files with a warning or blocker, or when the server asks for a fix", () => {
    expect(isFixable(base)).toBe(true);
    expect(isFixable({ ...base, compliance_state: "warning" })).toBe(true);
    expect(isFixable({ ...base, compliance_state: "clean", desired_action: "fix" })).toBe(true);
  });

  it("is false for clean, unverified and missing files", () => {
    expect(isFixable({ ...base, compliance_state: "clean" })).toBe(false);
    expect(isFixable({ ...base, compliance_state: "unknown" })).toBe(false);
    expect(isFixable({ ...base, discovery_state: "missing" })).toBe(false);
  });
});

describe("formatting", () => {
  it("formats sizes with one decimal only where it helps", () => {
    expect(formatSize(19)).toBe("19 B");
    expect(formatSize(2_500_000_000)).toBe("2.3 GB");
    expect(formatSize(10 * 1024 ** 3)).toBe("10 GB");
    expect(formatSize(-1)).toBe("—");
    expect(formatSize(Number.NaN)).toBe("—");
  });

  it("formats bitrates in Mbps and durations in hours and minutes", () => {
    expect(formatBitrate(12_129_150)).toBe("12.1 Mbps");
    expect(formatBitrate(undefined)).toBe("—");
    expect(formatDuration(7177)).toBe("1:59 h");
    expect(formatDuration(65)).toBe("1 min");
    expect(formatDuration(undefined)).toBe("—");
  });

  it("estimates minutes for a rewrite at a given throughput, never below one", () => {
    expect(estimateMinutes(12 * 1024 ** 3)).toBe(2);
    expect(estimateMinutes(1024)).toBe(1);
    expect(estimateMinutes(0)).toBe(0);
  });

  it("derives a readable title from the path", () => {
    expect(baseName("Film/Pretty Woman (1990)/Pretty Woman (1990).mp4")).toEqual({
      title: "Pretty Woman (1990)",
      folder: "Film/Pretty Woman (1990)",
    });
    expect(baseName("loose.mkv")).toEqual({ title: "loose", folder: "" });
    expect(baseName("")).toEqual({ title: "", folder: "" });
  });
});

describe("languageName", () => {
  it("maps ISO 639-2 codes to English names and falls back to the code", () => {
    expect(languageName("deu")).toBe("German");
    expect(languageName("ger")).toBe("German");
    expect(languageName("eng")).toBe("English");
    expect(languageName("xyz")).toBe("xyz");
    expect(languageName(undefined)).toBe("Unknown");
    expect(languageName("und")).toBe("Unknown");
  });
});

describe("summarizeInspection", () => {
  const ffprobe = {
    format: { format_name: "mov,mp4,m4a,3gp,3g2,mj2", duration: "7177.024", bit_rate: "12129150" },
    streams: [
      { index: 0, codec_type: "video", codec_name: "h264", width: 1920, height: 1080, disposition: { default: 1 }, tags: { language: "eng" } },
      { index: 1, codec_type: "audio", codec_name: "eac3", channels: 6, channel_layout: "5.1(side)", disposition: { default: 1 }, tags: { language: "deu" } },
      { index: 2, codec_type: "audio", codec_name: "aac", channels: 2, disposition: { default: 0 }, tags: { language: "deu" } },
      { index: 5, codec_type: "subtitle", codec_name: "mov_text", disposition: { default: 0, forced: 1 }, tags: { language: "deu" } },
      { index: 6, codec_type: "data", codec_name: "bin_data" },
    ],
  };

  it("reads container facts and one row per track, in file order", () => {
    const summary = summarizeInspection(ffprobe);

    expect(summary.container).toBe("mov,mp4,m4a,3gp,3g2,mj2");
    expect(summary.durationSeconds).toBeCloseTo(7177.024);
    expect(summary.bitrate).toBe(12_129_150);
    expect(summary.tracks.map((track) => track.kind)).toEqual(["video", "audio", "audio", "subtitle", "other"]);
    expect(summary.tracks[0]).toMatchObject({ codec: "h264", detail: "1920×1080", language: "eng", isDefault: true });
    expect(summary.tracks[1]).toMatchObject({ codec: "eac3", detail: "5.1", language: "deu", isDefault: true });
    expect(summary.tracks[2]).toMatchObject({ detail: "Stereo", isDefault: false });
    expect(summary.tracks[3]).toMatchObject({ isForced: true });
  });

  it("numbers audio and subtitle tracks per kind so tables can say A0, A1, S0", () => {
    const summary = summarizeInspection(ffprobe);

    expect(summary.tracks.map((track) => track.label)).toEqual(["V0", "A0", "A1", "S0", ""]);
  });

  it("survives missing or malformed parts without throwing", () => {
    expect(summarizeInspection({})).toEqual({ container: undefined, durationSeconds: undefined, bitrate: undefined, tracks: [] });
    expect(summarizeInspection({ format: "nope", streams: "nope" }).tracks).toEqual([]);
    expect(summarizeInspection({ streams: [null, 3, { codec_type: "audio" }] }).tracks).toHaveLength(1);
    expect(summarizeInspection({ format: { duration: "abc", bit_rate: "-5" } })).toMatchObject({
      durationSeconds: undefined,
      bitrate: undefined,
    });
  });
});


describe("pending subtitle files and fixability", () => {
  const clean = { discovery_state: "present", compliance_state: "clean", desired_action: "none" };

  it("is fixable once an external subtitle waits to be merged, even for a ready file", () => {
    expect(isFixable(clean)).toBe(false);
    expect(isFixable({ ...clean, pending_sidecars: 1 })).toBe(true);
    expect(isFixable({ ...clean, discovery_state: "missing", pending_sidecars: 1 })).toBe(false);
  });
});

describe("describeSidecar", () => {
  it("says what a fix does with each subtitle file", async () => {
    const { describeSidecar } = await import("../src/features/files/model");
    expect(describeSidecar("pending", "deu")).toBe("German: Prepare embeds it");
    expect(describeSidecar("embedded", "eng")).toMatch(/already has this subtitle/);
    expect(describeSidecar("ignored_language", "fra")).toMatch(/only German, English and the audio languages/);
    expect(describeSidecar("unknown_language", "und")).toMatch(/Film\.de\.srt/);
    expect(describeSidecar("unsupported", "und")).toMatch(/only \.srt/);
    expect(describeSidecar("unchecked", "deu")).toBe("German: not known yet whether Prepare embeds it. Re-inspect the file first.");
    expect(describeSidecar(undefined, undefined)).toBe("");
  });
});

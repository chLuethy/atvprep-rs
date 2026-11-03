import { describe, expect, it } from "vitest";
import { convertPlan, isMp4Like } from "../src/features/files/convertPlan";
import type { Track } from "../src/features/files/model";

const track = (kind: Track["kind"], codec: string, detail = ""): Track => ({ kind, index: 0, label: "", codec, detail, language: undefined, isDefault: false, isForced: false });

describe("isMp4Like", () => {
  it("knows the containers that are verified and fixed in place", () => {
    expect([".mp4", ".M4V", ".mov"].every(isMp4Like)).toBe(true);
    expect([".mkv", ".avi", ".webm", ""].some(isMp4Like)).toBe(false);
  });
});

describe("convertPlan", () => {
  it("copies H.264 up to 1080p and Apple-compatible audio", () => {
    const plan = convertPlan([track("video", "h264", "1920×1080"), track("audio", "eac3"), track("audio", "aac")]);
    expect(plan).toMatchObject({ videoEncode: false, audioEncode: false });
  });

  it("copies HEVC", () => {
    expect(convertPlan([track("video", "hevc", "3840×2160"), track("audio", "ac3")]).videoEncode).toBe(false);
  });

  it("encodes H.264 above 1080p and codecs Apple TV cannot play", () => {
    expect(convertPlan([track("video", "h264", "3840×2160")]).videoEncode).toBe(true);
    for (const codec of ["vp9", "av1", "vc1", "mpeg2video"]) expect(convertPlan([track("video", codec, "1920×1080")]).videoEncode).toBe(true);
  });

  it("re-encodes only the audio that needs it", () => {
    const plan = convertPlan([track("video", "h264", "1920×1080"), track("audio", "dts")]);
    expect(plan).toMatchObject({ videoEncode: false, audioEncode: true });
  });

  it("says nothing it does not know when the inspection has no tracks", () => {
    expect(convertPlan([])).toEqual({ videoEncode: false, audioEncode: false, lines: [] });
  });
});

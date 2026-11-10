import { describe, expect, it } from "vitest";
import { prepareConfirm, preparable } from "../src/features/files/preparePlan";
import type { Track } from "../src/features/files/model";

const video = (codec: string, detail: string): Track => ({ kind: "video", index: 0, label: "V0", codec, detail, language: undefined } as Track);

describe("prepareConfirm", () => {
  it("an MP4 without re-encode is rewritten in place and nothing is deleted", () => {
    const plan = prepareConfirm({ suffix: ".mp4", needs_reencode: false }, []);

    expect(plan.reencode).toBe(false);
    expect(plan.lines.join(" ")).not.toMatch(/delet/i);
    expect(plan.lines).toEqual(["Prepare rewrites this MP4 in place where needed. Files already at the target are left alone."]);
  });

  it("another container keeps its original while the setting is off", () => {
    const plan = prepareConfirm({ suffix: ".mkv" }, [], false);

    expect(plan.reencode).toBe(false);
    expect(plan.lines).toEqual(["Prepare writes an MP4 next to this .mkv file. The original is kept."]);
  });

  it("another container loses its original only after verification while the setting is on", () => {
    const plan = prepareConfirm({ suffix: ".mkv" }, [], true);

    expect(plan.lines).toEqual(["Prepare writes an MP4 next to this .mkv file. The original is removed once the new MP4 passes verification."]);
  });

  it("claims nothing about the original when the setting cannot be read", () => {
    const plan = prepareConfirm({ suffix: ".mkv" }, [], null);

    expect(plan.lines.join(" ")).toMatch(/cannot be read/);
    expect(plan.lines.join(" ")).not.toMatch(/is kept/);
  });

  it("a re-encode of another container also says what happens to the original", () => {
    const plan = prepareConfirm({ suffix: ".mkv", needs_reencode: true }, [], true);

    expect(plan.reencode).toBe(true);
    expect(plan.lines.join(" ")).toMatch(/Encode audio and video/);
    expect(plan.lines.join(" ")).toMatch(/original is removed once the new MP4 passes verification/);
  });

  it("an MP4 says nothing about removing an original, whatever the setting", () => {
    expect(prepareConfirm({ suffix: ".mp4" }, [], true).lines.join(" ")).not.toMatch(/removed/);
    expect(prepareConfirm({ suffix: ".mp4", needs_reencode: true }, [], true).lines.join(" ")).not.toMatch(/removed/);
  });

  it("a re-encode says which level it needs and that the file otherwise stays as it is", () => {
    const plan = prepareConfirm({ suffix: ".mp4", needs_reencode: true }, []);

    expect(plan.reencode).toBe(true);
    expect(plan.lines.join(" ")).toMatch(/Encode audio and video/);
    expect(plan.lines.join(" ")).toMatch(/otherwise the file stays as it is/);
    expect(plan.lines.join(" ")).not.toMatch(/delete_original/);
  });

  it("predicts a re-encode of another container from its codecs", () => {
    expect(prepareConfirm({ suffix: ".mkv" }, [video("vp9", "×1080")]).reencode).toBe(true);
    expect(prepareConfirm({ suffix: ".mkv" }, [video("h264", "×1080")]).reencode).toBe(false);
  });
});

describe("preparable", () => {
  const base = { discovery_state: "present", suffix: ".mp4", compliance_state: "warning", desired_action: "fix" };

  it("offers a file with something to do", () => {
    expect(preparable(base)).toBe(true);
    expect(preparable({ ...base, suffix: ".mkv", compliance_state: "unknown", desired_action: "convert" })).toBe(true);
  });

  it("offers a video that needs a re-encode: the preview asks for the release", () => {
    expect(preparable({ ...base, needs_reencode: true })).toBe(true);
    expect(preparable({ ...base, compliance_state: "clean", desired_action: "none", needs_reencode: true })).toBe(true);
  });

  it("still refuses a re-encode file that is missing or busy", () => {
    expect(preparable({ ...base, needs_reencode: true, discovery_state: "missing" })).toBe(false);
    expect(preparable({ ...base, needs_reencode: true, fix_status: "queued" })).toBe(false);
  });

  it("refuses missing files, busy files, kept conversions and ready files", () => {
    expect(preparable({ ...base, discovery_state: "missing" })).toBe(false);
    expect(preparable({ ...base, fix_status: "queued" })).toBe(false);
    expect(preparable({ ...base, suffix: ".mkv", converted_kept: true })).toBe(false);
    expect(preparable({ ...base, compliance_state: "clean", desired_action: "none" })).toBe(false);
  });

  it("still offers a ready file that has a subtitle waiting", () => {
    expect(preparable({ ...base, compliance_state: "clean", desired_action: "none", pending_sidecars: 1 })).toBe(true);
  });
});

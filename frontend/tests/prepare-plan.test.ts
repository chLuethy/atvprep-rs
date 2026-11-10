import { describe, expect, it } from "vitest";
import { changeLabel, needLabel, prepareConfirm, preparable } from "../src/features/files/preparePlan";

describe("prepareConfirm", () => {
  it("an MP4 without a need is rewritten in place and nothing is deleted", () => {
    const plan = prepareConfirm({ suffix: ".mp4", encode_need: "none" });

    expect(plan.reencode).toBe(false);
    expect(plan.lines.join(" ")).not.toMatch(/delet/i);
    expect(plan.lines).toEqual(["Prepare rewrites this MP4 in place where needed. Files already at the target are left alone."]);
  });

  it("another container keeps its original while the setting is off", () => {
    const plan = prepareConfirm({ suffix: ".mkv" }, false);

    expect(plan.reencode).toBe(false);
    expect(plan.lines).toEqual(["Prepare writes an MP4 next to this .mkv file. The original is kept."]);
  });

  it("another container loses its original only after verification while the setting is on", () => {
    const plan = prepareConfirm({ suffix: ".mkv" }, true);

    expect(plan.lines).toEqual(["Prepare writes an MP4 next to this .mkv file. The original is removed once the new MP4 passes verification."]);
  });

  it("claims nothing about the original when the setting cannot be read", () => {
    const plan = prepareConfirm({ suffix: ".mkv" }, null);

    expect(plan.lines.join(" ")).toMatch(/cannot be read/);
    expect(plan.lines.join(" ")).not.toMatch(/is kept/);
  });

  it("a re-encode of another container also says what happens to the original", () => {
    const plan = prepareConfirm({ suffix: ".mkv", encode_need: "video" }, true);

    expect(plan.reencode).toBe(true);
    expect(plan.lines.join(" ")).toMatch(/Encode audio and video/);
    expect(plan.lines.join(" ")).toMatch(/original is removed once the new MP4 passes verification/);
  });

  it("an MP4 says nothing about removing an original, whatever the setting", () => {
    expect(prepareConfirm({ suffix: ".mp4" }, true).lines.join(" ")).not.toMatch(/removed/);
    expect(prepareConfirm({ suffix: ".mp4", encode_need: "video" }, true).lines.join(" ")).not.toMatch(/removed/);
  });

  it("a re-encode says which level it needs and that the file otherwise stays as it is", () => {
    const plan = prepareConfirm({ suffix: ".mp4", encode_need: "video" });

    expect(plan.reencode).toBe(true);
    expect(plan.lines.join(" ")).toMatch(/Encode audio and video/);
    expect(plan.lines.join(" ")).toMatch(/otherwise the file stays as it is/);
    expect(plan.lines.join(" ")).not.toMatch(/delete_original/);
  });

  it("an audio need names its level and is no re-encode", () => {
    const plan = prepareConfirm({ suffix: ".mp4", encode_need: "audio" });

    expect(plan.reencode).toBe(false);
    expect(plan.lines.join(" ")).toMatch(/Encode audio/);
  });

  it("an unknown need claims nothing and points to the chosen level", () => {
    const plan = prepareConfirm({ suffix: ".mkv", encode_need: "unknown" });

    expect(plan.reencode).toBe(false);
    expect(plan.lines.join(" ")).toMatch(/not known yet/);
  });

  it("predicts nothing from a container or its codecs", () => {
    expect(prepareConfirm({ suffix: ".mkv" }).reencode).toBe(false);
    expect(prepareConfirm({ suffix: ".mkv", encode_need: "none" }).reencode).toBe(false);
  });
});

describe("the labels of need and change", () => {
  it("name what the server stored, and treat a missing value as unknown", () => {
    expect(needLabel("video")).toBe("Video");
    expect(needLabel(undefined)).toBe("Unknown");
    expect(changeLabel("audio")).toBe("Audio encoding");
    expect(changeLabel("reencode")).toBe("Re-encode");
    expect(changeLabel(undefined)).toBe("Unknown");
  });
});

describe("preparable", () => {
  const base = { discovery_state: "present", suffix: ".mp4", compliance_state: "warning", desired_action: "fix" };

  it("offers a file with something to do", () => {
    expect(preparable(base)).toBe(true);
    expect(preparable({ ...base, suffix: ".mkv", compliance_state: "unknown", desired_action: "convert" })).toBe(true);
  });

  it("offers a file that needs encoding: the preview asks for the release", () => {
    expect(preparable({ ...base, encode_need: "video" })).toBe(true);
    expect(preparable({ ...base, compliance_state: "clean", desired_action: "none", encode_need: "video" })).toBe(true);
    expect(preparable({ ...base, compliance_state: "clean", desired_action: "none", encode_need: "audio" })).toBe(true);
  });

  it("offers a file whose need is still unknown: the run enforces the limit", () => {
    expect(preparable({ ...base, compliance_state: "clean", desired_action: "none", encode_need: "unknown" })).toBe(true);
  });

  it("still refuses a file that needs encoding when it is missing or busy", () => {
    expect(preparable({ ...base, encode_need: "video", discovery_state: "missing" })).toBe(false);
    expect(preparable({ ...base, encode_need: "video", fix_status: "queued" })).toBe(false);
  });

  it("refuses missing files, busy files, kept conversions and ready files", () => {
    expect(preparable({ ...base, discovery_state: "missing" })).toBe(false);
    expect(preparable({ ...base, fix_status: "queued" })).toBe(false);
    expect(preparable({ ...base, suffix: ".mkv", converted_kept: true })).toBe(false);
    expect(preparable({ ...base, compliance_state: "clean", desired_action: "none", encode_need: "none" })).toBe(false);
  });

  it("still offers a ready file that has a subtitle waiting", () => {
    expect(preparable({ ...base, compliance_state: "clean", desired_action: "none", pending_sidecars: 1 })).toBe(true);
  });
});

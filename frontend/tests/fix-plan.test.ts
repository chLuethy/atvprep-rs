import { describe, expect, it } from "vitest";
import { blockerVerdict, expectedChanges, fixesAnything, summarizeChanges } from "../src/features/files/fixPlan";
import { parseReason } from "../src/features/files/model";

const changes = (...reasons: string[]) => expectedChanges(reasons.map(parseReason));

describe("expectedChanges: what a fix does for each verify reason", () => {
  it("patches header-level rules without re-encoding", () => {
    expect(changes("[blocker] ftyp-dby1-requires-atmos: ftyp lists dby1")).toEqual([
      { kind: "patch", rule: "ftyp-dby1-requires-atmos", text: "Remove the dby1 brand from the file header" },
    ]);
    expect(changes("[blocker] single-enabled-per-type: two enabled audio tracks")[0]).toMatchObject({ kind: "patch", text: "Enable one video track, the stereo audio and only its forced subtitle" });
    expect(changes("[blocker] subtitle-handler-type: handler is text")[0]).toMatchObject({ kind: "patch", text: "Mark subtitle tracks as subtitles (sbtl)" });
  });

  it("turns each apple-tags finding into the concrete write it implies", () => {
    expect(changes(
      "[warning] apple-tags: 4 tracks lack an elng tag",
      "[warning] apple-tags: 1 surround track has no stereo fallback",
      "[warning] apple-tags: 2 audio tracks do not follow their forced subtitle track",
      "[warning] apple-tags: 1 subtitle track is not linked to its forced variant",
      "[warning] apple-tags: 1 forced subtitle track lacks the forced display flags",
    ).map((change) => change.text)).toEqual([
      "Write 4 elng language tags",
      "Link 1 surround track to its stereo twin (fall)",
      "Link 2 audio tracks to the forced subtitle (folw)",
      "Link 1 subtitle track to its forced variant (forc)",
      "Set the forced display flags on 1 subtitle track",
    ]);
  });

  it("handles singular elng and a layout mismatch", () => {
    expect(changes("[warning] apple-tags: 1 track lacks an elng tag")[0]?.text).toBe("Write 1 elng language tag");
    expect(changes("[warning] apple-tags: track references differ from the expected layout")[0]).toMatchObject({
      kind: "patch",
      text: "Rewrite the track references to the expected layout",
    });
  });

  it("falls back to the finding's own words for an apple-tags message it does not know", () => {
    expect(changes("[warning] apple-tags: something new")[0]).toMatchObject({ kind: "patch", text: "something new" });
  });

  it("remuxes the file for layout problems a patch cannot fix", () => {
    expect(changes("[warning] fast-start: moov atom appears after mdat")[0]).toMatchObject({ kind: "remux", text: "Rewrite the file with the index at the front (faststart)" });
    expect(changes("[blocker] no-fragments: fragmented MP4")[0]).toMatchObject({ kind: "remux", text: "Rewrite the fragmented file as a regular MP4" });
  });

  it("says plainly what a fix does not change", () => {
    const [level] = changes("[warning] avc-level-atv-limit: H.264 level 6.0 exceeds 5.2");
    expect(level).toMatchObject({ kind: "none", rule: "avc-level-atv-limit" });
    expect(level?.text).toContain("H.264 level 6.0 exceeds 5.2");
    expect(level?.text).toMatch(/needs a re-encode/i);
    expect(changes("[warning] hevc-level-atv-limit: level 6.1")[0]?.kind).toBe("none");
    expect(changes("[warning] audio-sample-rate: 96 kHz")[0]?.kind).toBe("none");
  });

  it("does not claim to know a rule it has never seen", () => {
    const [unknown] = changes("[warning] brand-new-rule: something");
    expect(unknown).toMatchObject({ kind: "none", rule: "brand-new-rule" });
    expect(unknown?.text).toMatch(/effect of a fix is unknown/i);
  });

  it("ignores free-text reasons that are not violations", () => {
    expect(changes("No compliance violations reported by verify.", "Current ffprobe inspection is available, but AVPlayer compliance requires a verify report.")).toEqual([]);
  });
});

describe("expectedChanges: new and corrected mappings", () => {
  it("rewrites the file for a missing dby1 brand and removes the brand otherwise", () => {
    expect(changes("[blocker] ftyp-dby1-requires-atmos: an audio track reports real Dolby Atmos (JOC) but ftyp compatible_brands is missing 'dby1'; AVPlayer may not enable Atmos passthrough to the receiver.")[0])
      .toMatchObject({ kind: "remux", text: "Rewrite the file so its header carries the dby1 brand" });
    expect(changes("[blocker] ftyp-dby1-requires-atmos: ftyp lists dby1")[0]?.kind).toBe("patch");
    // The dec3 flag claims Atmos the stream lacks: prepare clears it in place, it does not remux.
    expect(changes("[blocker] ftyp-dby1-requires-atmos: an audio track's dec3 box claims Dolby Atmos (JOC) but the stream carries none; AVPlayer would announce Atmos the receiver never gets.")[0])
      .toMatchObject({ kind: "patch", text: "Clear the Atmos flag the audio track claims without carrying Atmos" });
  });

  it("patches the enabled tracks to Apple's selection", () => {
    expect(changes("[warning] default-track-selection: track selection differs from Apple's: enable deu stereo audio, deu forced subtitle; disable deu 5.1 audio, deu full subtitle")[0])
      .toMatchObject({ kind: "patch", text: "Enable the tracks Apple enables: stereo audio and its forced subtitle" });
  });

  it("reports a crashed rule as a failed check, whatever rule it came from", () => {
    expect(changes("[blocker] fast-start: rule crashed: ffprobe not found")[0])
      .toMatchObject({ kind: "none", text: "The check itself failed (rule crashed: ffprobe not found); check the file again" });
  });
});

describe("expectedChanges: what Prepare adds", () => {
  it("adds a stereo track, drops a duplicate audio track and re-encodes a video that has to be", () => {
    expect(changes("[warning] stereo-twin: 1 language has surround only")[0])
      .toEqual({ kind: "remux", rule: "stereo-twin", text: "Add an AAC stereo track for the surround-only language" });
    expect(changes("[warning] duplicate-audio: two identical tracks")[0])
      .toEqual({ kind: "remux", rule: "duplicate-audio", text: "Drop the duplicate audio track" });
    expect(changes("[blocker] video-reencode: the video needs a re-encode")[0])
      .toEqual({ kind: "reencode", rule: "video-reencode", text: "Re-encode the video to HEVC (lossy, needs confirmation)" });
  });

  it("reports a needed re-encode in the summary and counts it as resolved by Prepare", () => {
    const summary = summarizeChanges(changes("[blocker] video-reencode: x", "[warning] avc-level-atv-limit: level 6.0"));

    expect(summary).toEqual({ resolved: 1, remaining: 1, remux: false, reencode: true });
  });

  it("calls a blocker that only needs a re-encode fixable", () => {
    expect(blockerVerdict(["[blocker] video-reencode: x"].map(parseReason))).toBe("fixable");
  });
});

describe("blockerVerdict", () => {
  const verdict = (...reasons: string[]) => blockerVerdict(reasons.map(parseReason));

  it("is fixable when every blocker has a fix", () => {
    expect(verdict("[blocker] no-fragments: fragmented", "[blocker] subtitle-handler-type: text")).toBe("fixable");
  });

  it("is manual when a blocker needs a re-encode", () => {
    expect(verdict("[blocker] no-fragments: fragmented", "[blocker] avc-level-atv-limit: level 6.0")).toBe("manual");
  });

  it("is check-failed when a rule crashed", () => {
    expect(verdict("[blocker] avc-level-atv-limit: level 6.0", "[blocker] fast-start: rule crashed: boom")).toBe("check-failed");
  });

  it("ignores warnings", () => {
    expect(verdict("[blocker] no-fragments: fragmented", "[warning] avc-level-atv-limit: level 6.0")).toBe("fixable");
  });
});

describe("summarizeChanges", () => {
  it("counts what is resolved and what stays, and whether the file is rewritten by a remux", () => {
    const summary = summarizeChanges(changes(
      "[warning] apple-tags: 4 tracks lack an elng tag",
      "[warning] fast-start: moov after mdat",
      "[warning] avc-level-atv-limit: level 6.0",
    ));

    expect(summary).toEqual({ resolved: 2, remaining: 1, remux: true, reencode: false });
  });

  it("reports a patch-only fix as no remux", () => {
    expect(summarizeChanges(changes("[blocker] ftyp-dby1-requires-atmos: x"))).toEqual({ resolved: 1, remaining: 0, remux: false, reencode: false });
  });
});

describe("fixesAnything", () => {
  const base = { compliance_state: "warning", desired_action: "fix" };

  it("is true when at least one reason is something a fix resolves", () => {
    expect(fixesAnything({ ...base, compliance_reasons: ["[warning] apple-tags: 1 track lacks an elng tag", "[warning] audio-sample-rate: 96 kHz"] })).toBe(true);
  });

  it("is false when every reason is one a fix cannot change", () => {
    expect(fixesAnything({ ...base, compliance_reasons: ["[warning] avc-level-atv-limit: level 6.0"] })).toBe(false);
  });

  it("trusts the server when it asks for a fix but gave no violation to read", () => {
    expect(fixesAnything({ ...base, compliance_reasons: [] })).toBe(true);
    expect(fixesAnything({ ...base, compliance_reasons: undefined })).toBe(true);
    expect(fixesAnything({ ...base, compliance_reasons: ["Current ffprobe inspection is available, but AVPlayer compliance requires a verify report."] })).toBe(true);
  });
});

describe("external subtitles make a file fixable", () => {
  it("offers a fix for a file with nothing to repair when a subtitle file is waiting to be merged", () => {
    expect(fixesAnything({ compliance_reasons: [], pending_sidecars: 1 })).toBe(true);
    expect(fixesAnything({ compliance_reasons: ["[warning] apple-tags: 4 tracks lack an elng tag"], pending_sidecars: 0 })).toBe(true);
  });

  it("still refuses a fix that only has unfixable findings and no subtitle to merge", () => {
    expect(fixesAnything({ compliance_reasons: ["[blocker] avc-level-atv-limit: level 5.2"], pending_sidecars: 0 })).toBe(false);
  });
});

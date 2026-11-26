import { describe, expect, it } from "vitest";
import { parseLog } from "../src/features/activity/logModel";

const prepare = [
  "[00:00:01] Start: prepare",
  "[00:00:01] File: TV/Madam Secretary (2014)/Season 03/Madam Secretary (2014) - S03E20 - Extraordinary Hazard [WEBDL-1080p][8bit][h264][AC3 5.1][DE+EN]-TvR.mp4",
  "[00:00:01] Tool: atvprep 0.9.0",
  "[00:00:01] Encode allowance: copy",
  "[00:00:01] Target profile: crf 18, prefer HEVC 10-bit no, keep AV1 no, fix DTS no",
  "[00:00:01] Delete original: yes",
  "ATVPREP-PROGRESS 87",
  "ATVPREP-PROGRESS 90",
  "ATVPREP-PROGRESS 99",
  '{"bytes":3645981583,"phase":"verify","seconds":71.2}',
  "[00:14:20] Phase verify: started 00:14:19, took 1:11",
  "postfix-only: Madam Secretary - S03E20",
  "  • default-track-selection: track selection fixed",
  "  • tags: elng 2 → elng 7 · fall 2",
  "ATVPREP-VERIFY passed, 0 blockers, 0 warnings",
  "ATVPREP-OUTCOME patched",
  "1 file(s) repaired in place",
  "[00:14:21] Exit code: 0",
  "[00:14:21] Duration: 14:21",
].join("\n");

describe("parseLog", () => {
  it("collects the opening facts apart from the steps", () => {
    const doc = parseLog(prepare);
    expect(doc.facts.map((fact) => fact.key)).toEqual(["Start", "File", "Tool", "Encode allowance", "Target profile", "Delete original"]);
    expect(doc.facts[0]).toEqual({ key: "Start", value: "prepare" });
  });

  it("folds the progress lines into one value", () => {
    const doc = parseLog(prepare);
    expect(doc.progress).toBe(99);
    expect(doc.rows.some((row) => row.kind === "note" && row.text.startsWith("ATVPREP-PROGRESS"))).toBe(false);
  });

  it("joins a phase's timing record and its step line into one row", () => {
    const phases = parseLog(prepare).rows.filter((row) => row.kind === "phase");
    expect(phases).toEqual([{ kind: "phase", name: "verify", started: "00:14:19", took: "1:11", bytes: 3645981583 }]);
  });

  it("reads the check, the outcome and the end of the run", () => {
    const rows = parseLog(prepare).rows;
    expect(rows).toContainEqual({ kind: "check", passed: true, blockers: 0, warnings: 0 });
    expect(rows).toContainEqual({ kind: "outcome", value: "patched" });
    expect(rows).toContainEqual({ kind: "end", exit: "0", duration: "14:21" });
  });

  it("keeps what it does not know as notes, bullets apart", () => {
    const notes = parseLog(prepare).rows.filter((row) => row.kind === "note");
    expect(notes.map((note) => note.kind === "note" && [note.text, note.bullet])).toEqual([
      ["postfix-only: Madam Secretary - S03E20", false],
      ["default-track-selection: track selection fixed", true],
      ["tags: elng 2 → elng 7 · fall 2", true],
      ["1 file(s) repaired in place", false],
    ]);
  });

  it("reads a phase timing record without a step line", () => {
    expect(parseLog('{"phase":"remux","seconds":2.5}').rows).toEqual([{ kind: "phase", name: "remux", took: "2.50 s" }]);
  });

  it("reads rules of a check and keeps the detail of a warning", () => {
    const rows = parseLog("Rule fast-start: warning: moov at end\nRule apple-tags: ok\nState: warning\nExit code: 1").rows;
    expect(rows[0]).toEqual({ kind: "rule", name: "fast-start", state: "warning", detail: "moov at end" });
    expect(rows[1]).toEqual({ kind: "rule", name: "apple-tags", state: "ok", detail: "" });
  });

  it("reads the streams of an inspection with their channels, language and flags", () => {
    const rows = parseLog("Start: inspect\nStream 0: video h264, default\nStream 1: audio eac3, 6 channels, 48000 Hz, language ger, default").rows;
    expect(rows).toEqual([
      { kind: "stream", index: "0", type: "video", codec: "h264", parts: ["default"] },
      { kind: "stream", index: "1", type: "audio", codec: "eac3", parts: ["6 channels", "48000 Hz", "language ger", "default"] },
    ]);
  });

  it("reads a rule that names a track", () => {
    expect(parseLog("Rule stereo-twin (track 2): warning: no stereo twin").rows).toEqual([
      { kind: "rule", name: "stereo-twin", track: "2", state: "warning", detail: "no stereo twin" },
    ]);
  });

  it("reads a failed check: tool output stays a note, the failure is an error", () => {
    const doc = parseLog("Start: verify\nboom\nExit code: 2\nFailed: verification_failed: the tool exited with code 2");
    expect(doc.rows.map((row) => row.kind)).toEqual(["note", "end", "error"]);
  });

  it("reads a failure and the plan lines", () => {
    const rows = parseLog("ATVPREP-PLAN video: copy\nATVPREP-ERROR disk full\nFailed: verification_failed: disk full").rows;
    expect(rows).toEqual([
      { kind: "plan", stream: "video", action: "copy" },
      { kind: "error", text: "disk full" },
      { kind: "error", text: "Failed: verification_failed: disk full" },
    ]);
  });

  it("marks where the stored log omits its middle", () => {
    expect(parseLog("a\n… 1234 bytes omitted …\nb").rows).toEqual([
      { kind: "note", text: "a", bullet: false },
      { kind: "omitted", bytes: 1234 },
      { kind: "note", text: "b", bullet: false },
    ]);
  });

  it("does not throw on broken or cut lines and loses none of them", () => {
    const doc = parseLog('{"phase":\n[00:00:01\nATVPREP-PROGRESS\n\r\n   \nlog line');
    expect(doc.rows.map((row) => (row.kind === "note" ? row.text : row.kind))).toEqual(['{"phase":', "[00:00:01", "ATVPREP-PROGRESS", "log line"]);
  });

  it("returns nothing for an empty log", () => {
    expect(parseLog("")).toEqual({ facts: [], rows: [], progress: undefined });
  });
});

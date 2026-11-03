import { describe, expect, it } from "vitest";
import { hiddenSet, knownState, normalizeHidden, setGroupHidden, STATE_FILTERS, TRAIT_FILTERS, toggleHidden } from "../src/features/files/filters";

describe("library filter list", () => {
  it("keeps only known states, once each, always in the same order", () => {
    expect(normalizeHidden("reencode,clean,nonsense,clean,,blocker")).toBe("clean,blocker,reencode");
    expect(normalizeHidden(undefined)).toBe("");
    expect(normalizeHidden(new Set(["missing", "warning"]))).toBe("warning,missing");
  });

  it("flips one entry between shown and hidden", () => {
    expect(toggleHidden("", "clean")).toBe("clean");
    expect(toggleHidden("clean", "warning")).toBe("clean,warning");
    expect(toggleHidden("clean,warning", "clean")).toBe("warning");
    expect([...hiddenSet("clean,warning")]).toEqual(["clean", "warning"]);
  });

  it("shows or hides a whole group and leaves the other one alone", () => {
    expect(setGroupHidden("reencode", STATE_FILTERS, true)).toBe("clean,warning,blocker,unknown,probe_failed,missing,reencode");
    expect(setGroupHidden("clean,blocker,reencode", STATE_FILTERS, false)).toBe("reencode");
    expect(setGroupHidden("clean,reencode", TRAIT_FILTERS, false)).toBe("clean");
  });

  it("knows a single state a link may ask for", () => {
    expect(knownState("convert")).toBe("convert");
    expect(knownState("nonsense")).toBe("");
    expect(knownState(undefined)).toBe("");
  });
});

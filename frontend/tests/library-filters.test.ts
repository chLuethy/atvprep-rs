import { describe, expect, it } from "vitest";
import { hasSet, hiddenSet, knownState, normalizeHas, normalizeHidden, setGroupHidden, STATE_FILTERS, TRAIT_FILTERS, toggleHas, toggleHidden } from "../src/features/files/filters";

describe("library filter list", () => {
  it("keeps only known states, once each, always in the same order", () => {
    expect(normalizeHidden("clean,nonsense,clean,,blocker")).toBe("clean,blocker");
    expect(normalizeHidden(undefined)).toBe("");
    expect(normalizeHidden(new Set(["missing", "warning"]))).toBe("warning,missing");
  });

  it("offers Embed open next to the other file traits", () => {
    expect(TRAIT_FILTERS.map((entry) => entry.token)).toContain("embed");
    expect(knownState("embed")).toBe("embed");
  });

  it("drops properties from an old hide list, since hiding them is not what a property filter does now", () => {
    expect(normalizeHidden("embed,clean,reencode")).toBe("clean");
    expect(normalizeHidden("reencode")).toBe("");
  });

  it("keeps the chosen properties as a canonical list of known properties only", () => {
    expect(normalizeHas("embed,clean,reencode,nonsense,embed")).toBe("reencode,embed");
    expect(normalizeHas(undefined)).toBe("");
    expect(toggleHas("", "embed")).toBe("embed");
    expect(toggleHas("embed", "convert")).toBe("convert,embed");
    expect(toggleHas("convert,embed", "convert")).toBe("embed");
    expect([...hasSet("reencode,embed")]).toEqual(["reencode", "embed"]);
  });

  it("flips one entry between shown and hidden", () => {
    expect(toggleHidden("", "clean")).toBe("clean");
    expect(toggleHidden("clean", "warning")).toBe("clean,warning");
    expect(toggleHidden("clean,warning", "clean")).toBe("warning");
    expect([...hiddenSet("clean,warning")]).toEqual(["clean", "warning"]);
  });

  it("shows or hides every state at once", () => {
    expect(setGroupHidden("", STATE_FILTERS, true)).toBe("clean,warning,blocker,unknown,probe_failed,missing");
    expect(setGroupHidden("clean,blocker", STATE_FILTERS, false)).toBe("");
  });

  it("knows a single state a link may ask for", () => {
    expect(knownState("convert")).toBe("convert");
    expect(knownState("nonsense")).toBe("");
    expect(knownState(undefined)).toBe("");
  });
});

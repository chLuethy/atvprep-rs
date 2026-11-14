import { describe, expect, it } from "vitest";
import { hasSet, hiddenSet, knownState, normalizeHas, normalizeHidden, setGroupHidden, STATE_FILTERS, TRAIT_FILTERS, toggleHas, toggleHidden } from "../src/features/files/filters";

describe("library filter list", () => {
  it("keeps only known states, once each, always in the same order", () => {
    expect(normalizeHidden("clean,nonsense,clean,,blocker")).toBe("clean,blocker");
    expect(normalizeHidden(undefined)).toBe("");
    expect(normalizeHidden(new Set(["missing", "warning"]))).toBe("warning,missing");
  });

  it("offers Subtitles embeddable next to the other file traits", () => {
    expect(TRAIT_FILTERS.find((entry) => entry.token === "embed")?.label).toBe("Subtitles embeddable");
    expect(knownState("embed")).toBe("embed");
  });

  it("has Embedding open as a state of its own, right after Ready", () => {
    expect(STATE_FILTERS.map((entry) => entry.token)).toEqual(["clean", "embed_open", "warning", "blocker", "unknown", "probe_failed", "missing"]);
    expect(STATE_FILTERS.find((entry) => entry.token === "embed_open")?.label).toBe("Embedding open");
    expect(knownState("embed_open")).toBe("embed_open");
    expect(normalizeHidden("missing,embed_open,clean")).toBe("clean,embed_open,missing");
  });

  it("drops properties from an old hide list, since hiding them is not what a property filter does now", () => {
    expect(normalizeHidden("embed,clean,encode_video")).toBe("clean");
    expect(normalizeHidden("encode_video")).toBe("");
  });

  it("keeps the chosen properties as a canonical list of known properties only", () => {
    expect(normalizeHas("embed,clean,encode_video,nonsense,embed")).toBe("encode_video,embed");
    expect(normalizeHas(undefined)).toBe("");
    expect(toggleHas("", "embed")).toBe("embed");
    expect(toggleHas("embed", "convert")).toBe("convert,embed");
    expect(toggleHas("convert,embed", "convert")).toBe("embed");
    expect([...hasSet("encode_video,embed")]).toEqual(["encode_video", "embed"]);
  });

  it("flips one entry between shown and hidden", () => {
    expect(toggleHidden("", "clean")).toBe("clean");
    expect(toggleHidden("clean", "warning")).toBe("clean,warning");
    expect(toggleHidden("clean,warning", "clean")).toBe("warning");
    expect([...hiddenSet("clean,warning")]).toEqual(["clean", "warning"]);
  });

  it("shows or hides every state at once", () => {
    expect(setGroupHidden("", STATE_FILTERS, true)).toBe("clean,embed_open,warning,blocker,unknown,probe_failed,missing");
    expect(setGroupHidden("clean,blocker", STATE_FILTERS, false)).toBe("");
  });

  it("knows a single state a link may ask for", () => {
    expect(knownState("convert")).toBe("convert");
    expect(knownState("nonsense")).toBe("");
    expect(knownState(undefined)).toBe("");
  });
});

import { describe, expect, it } from "vitest";
import { EMPTY_FILTER, fileFilterQuery, isFiltered, parseFileFilter, STATE_FILTERS, TRAIT_FILTERS } from "../src/features/files/filters";

const parse = (search: string) => parseFileFilter(new URLSearchParams(search));

describe("files filter: the entries", () => {
  it("has Embedding open as a state of its own, right after Ready", () => {
    expect(STATE_FILTERS.map((entry) => entry.token)).toEqual(["clean", "embed_open", "warning", "blocker", "unknown", "probe_failed", "missing"]);
    expect(STATE_FILTERS.find((entry) => entry.token === "embed_open")?.label).toBe("Embedding open");
  });

  it("offers Subtitles embeddable next to the other file properties", () => {
    expect(TRAIT_FILTERS.map((entry) => entry.token)).toEqual(["convert", "encode_audio", "encode_video", "embed", "converted"]);
    expect(TRAIT_FILTERS.find((entry) => entry.token === "embed")?.label).toBe("Subtitles embeddable");
  });
});

describe("files filter: the address", () => {
  it("reads the chosen states, properties and the search", () => {
    expect(parse("status=blocker,warning&has=embed&q=Heat")).toEqual({ q: "Heat", status: ["warning", "blocker"], has: ["embed"] });
    expect(parse("")).toEqual(EMPTY_FILTER);
  });

  it("keeps only known values, once each, always in the same order", () => {
    expect(parse("status=missing,nonsense,clean,clean&has=embed,clean,convert,embed")).toEqual({ q: "", status: ["clean", "missing"], has: ["convert", "embed"] });
  });

  it("writes the canonical query: states, then properties, then the search, nothing when empty", () => {
    expect(fileFilterQuery({ q: "", status: ["blocker", "warning"], has: ["embed", "convert"] })).toBe("status=warning,blocker&has=convert,embed");
    expect(fileFilterQuery({ q: "Pretty Woman & Co", status: [], has: [] })).toBe("q=Pretty%20Woman%20%26%20Co");
    expect(fileFilterQuery(EMPTY_FILTER)).toBe("");
  });

  it("reads back what it writes", () => {
    const filter = { q: "a,b c", status: ["clean", "missing"], has: ["converted"] };
    expect(parse(fileFilterQuery(filter))).toEqual(filter);
  });

  it("trims the search and drops an empty one", () => {
    expect(parse("q=%20%20").q).toBe("");
    expect(parse("q=%20Heat%20").q).toBe("Heat");
  });
});

describe("files filter: old addresses", () => {
  it("turns hide into the states that were still shown", () => {
    expect(parse("hide=clean,blocker").status).toEqual(["embed_open", "warning", "unknown", "probe_failed", "missing"]);
    expect(parse("hide=nonsense").status).toEqual([]);
  });

  it("drops properties an old hide list named, since hiding a property is not a thing", () => {
    expect(parse("hide=encode_video").status).toEqual([]);
    expect(parse("hide=encode_video").has).toEqual([]);
  });

  it("reads an old single status that names a property as that property", () => {
    expect(parse("status=encode_video")).toEqual({ q: "", status: [], has: ["encode_video"] });
    expect(parse("status=embed&has=convert")).toEqual({ q: "", status: [], has: ["convert", "embed"] });
  });

  it("keeps an old single state, and leaves out of it what hide hid", () => {
    expect(parse("status=blocker").status).toEqual(["blocker"]);
    expect(parse("status=blocker&hide=missing").status).toEqual(["blocker"]);
    expect(parse("status=convert&hide=missing")).toEqual({ q: "", status: ["clean", "embed_open", "warning", "blocker", "unknown", "probe_failed"], has: ["convert"] });
  });

  it("writes an old address the new way", () => {
    expect(fileFilterQuery(parse("hide=clean&has=embed"))).toBe("status=embed_open,warning,blocker,unknown,probe_failed,missing&has=embed");
  });
});

describe("files filter: is anything filtered", () => {
  it("counts states, properties and the search", () => {
    expect(isFiltered(EMPTY_FILTER)).toBe(false);
    expect(isFiltered({ ...EMPTY_FILTER, q: "x" })).toBe(true);
    expect(isFiltered({ ...EMPTY_FILTER, status: ["clean"] })).toBe(true);
    expect(isFiltered({ ...EMPTY_FILTER, has: ["embed"] })).toBe(true);
  });
});

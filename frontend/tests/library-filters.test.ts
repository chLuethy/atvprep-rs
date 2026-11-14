import { describe, expect, it } from "vitest";
import { activeQuickFilter, EMPTY_FILTER, fileFilterQuery, fileSortQuery, isFiltered, parseFileFilter, parseFileSort, QUICK_FILTERS, STATE_FILTERS, TRAIT_FILTERS } from "../src/features/files/filters";

const parse = (search: string) => parseFileFilter(new URLSearchParams(search));
const filterOf = (over: Partial<typeof EMPTY_FILTER>) => ({ ...EMPTY_FILTER, ...over });

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
    expect(parse("status=blocker,warning&has=embed&q=Heat")).toEqual(filterOf({ q: "Heat", status: ["warning", "blocker"], has: ["embed"] }));
    expect(parse("")).toEqual(EMPTY_FILTER);
  });

  it("keeps only known values, once each, always in the same order", () => {
    expect(parse("status=missing,nonsense,clean,clean&has=embed,clean,convert,embed")).toEqual(filterOf({ status: ["clean", "missing"], has: ["convert", "embed"] }));
  });

  it("writes the canonical query: states, then properties, then the search, nothing when empty", () => {
    expect(fileFilterQuery(filterOf({ status: ["blocker", "warning"], has: ["embed", "convert"] }))).toBe("status=warning,blocker&has=convert,embed");
    expect(fileFilterQuery(filterOf({ q: "Pretty Woman & Co" }))).toBe("q=Pretty%20Woman%20%26%20Co");
    expect(fileFilterQuery(EMPTY_FILTER)).toBe("");
  });

  it("reads back what it writes", () => {
    const filter = filterOf({ q: "a,b c", status: ["clean", "missing"], has: ["converted"] });
    expect(parse(fileFilterQuery(filter))).toEqual(filter);
  });

  it("trims the search and drops an empty one", () => {
    expect(parse("q=%20%20").q).toBe("");
    expect(parse("q=%20Heat%20").q).toBe("Heat");
  });
});

describe("files filter: library, extension and subtitle files", () => {
  it("reads and writes them in a fixed order after the properties", () => {
    const filter = parse("has_sidecar=false&suffix=.mp4,.mkv,.mp4&root_id=r_b,r_a&status=warning");
    expect(filter).toEqual(filterOf({ status: ["warning"], root: ["r_a", "r_b"], suffix: [".mkv", ".mp4"], sidecar: false }));
    expect(fileFilterQuery(filter)).toBe("status=warning&root_id=r_a,r_b&suffix=.mkv,.mp4&has_sidecar=false");
    expect(parse(fileFilterQuery(filter))).toEqual(filter);
  });

  it("drops a subtitle choice it does not know", () => {
    expect(parse("has_sidecar=maybe").sidecar).toBeNull();
    expect(parse("has_sidecar=true").sidecar).toBe(true);
  });

  it("counts them as filtering", () => {
    expect(isFiltered(filterOf({ root: ["r_a"] }))).toBe(true);
    expect(isFiltered(filterOf({ suffix: [".avi"] }))).toBe(true);
    expect(isFiltered(filterOf({ sidecar: false }))).toBe(true);
  });
});

describe("files filter: the sort in the address", () => {
  const sortOf = (search: string) => parseFileSort(new URLSearchParams(search));

  it("is recently added, oldest first, when the address says nothing", () => {
    expect(sortOf("")).toBeNull();
    expect(fileSortQuery(null)).toBe("");
    expect(sortOf("sort=added")).toBeNull();
    expect(sortOf("sort=nonsense&dir=desc")).toBeNull();
  });

  it("reads and writes a key and the direction, descending only when it is", () => {
    expect(sortOf("sort=size&dir=desc")).toEqual({ key: "size", dir: -1 });
    expect(sortOf("sort=title")).toEqual({ key: "title", dir: 1 });
    expect(sortOf("sort=added&dir=desc")).toEqual({ key: "added", dir: -1 });
    expect(fileSortQuery({ key: "status", dir: 1 })).toBe("sort=status");
    expect(fileSortQuery({ key: "added", dir: -1 })).toBe("sort=added&dir=desc");
    expect(fileSortQuery({ key: "added", dir: 1 })).toBe("");
  });
});

describe("files filter: quick filters", () => {
  const quick = (id: string) => QUICK_FILTERS.find((entry) => entry.id === id)!;

  it("sets the agreed states and nothing else", () => {
    expect(QUICK_FILTERS.map((entry) => entry.id)).toEqual(["attention", "unverified", "preparable"]);
    expect(fileFilterQuery(quick("attention").filter)).toBe("status=blocker,probe_failed,missing");
    expect(fileFilterQuery(quick("unverified").filter)).toBe("status=unknown");
    // Missing is a state of its own, so "without missing" needs no rule of its own.
    expect(fileFilterQuery(quick("preparable").filter)).toBe("status=embed_open,warning,blocker");
  });

  it("is active only while the filter is exactly it", () => {
    expect(activeQuickFilter(parse("status=missing,blocker,probe_failed"))?.id).toBe("attention");
    expect(activeQuickFilter(parse("status=blocker,probe_failed,missing&q=x"))).toBeUndefined();
    expect(activeQuickFilter(parse("status=unknown&has=embed"))).toBeUndefined();
    expect(activeQuickFilter(EMPTY_FILTER)).toBeUndefined();
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
    expect(parse("status=encode_video")).toEqual(filterOf({ has: ["encode_video"] }));
    expect(parse("status=embed&has=convert")).toEqual(filterOf({ has: ["convert", "embed"] }));
  });

  it("keeps an old single state, and leaves out of it what hide hid", () => {
    expect(parse("status=blocker").status).toEqual(["blocker"]);
    expect(parse("status=blocker&hide=missing").status).toEqual(["blocker"]);
    expect(parse("status=convert&hide=missing")).toEqual(filterOf({ status: ["clean", "embed_open", "warning", "blocker", "unknown", "probe_failed"], has: ["convert"] }));
  });

  it("writes an old address the new way", () => {
    expect(fileFilterQuery(parse("hide=clean&has=embed"))).toBe("status=embed_open,warning,blocker,unknown,probe_failed,missing&has=embed");
  });
});

describe("files filter: is anything filtered", () => {
  it("counts states, properties and the search", () => {
    expect(isFiltered(EMPTY_FILTER)).toBe(false);
    expect(isFiltered(filterOf({ q: "x" }))).toBe(true);
    expect(isFiltered(filterOf({ status: ["clean"] }))).toBe(true);
    expect(isFiltered(filterOf({ has: ["embed"] }))).toBe(true);
  });
});

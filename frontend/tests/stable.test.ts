import { describe, expect, it } from "vitest";
import { keepIfSame, reuseUnchanged, sameIgnoringIds } from "../src/lib/stable";

describe("sameIgnoringIds", () => {
  it("treats data as equal when only the ids differ, which the server changes with every response", () => {
    expect(sameIgnoringIds({ id: "a", root_id: "r1", size: 1, nested: { media_id: "x", ok: true } }, { id: "b", root_id: "r2", size: 1, nested: { media_id: "y", ok: true } })).toBe(true);
    expect(sameIgnoringIds([{ id: "a", n: 1 }, { id: "b", n: 2 }], [{ id: "c", n: 1 }, { id: "d", n: 2 }])).toBe(true);
  });

  it("notices every other difference", () => {
    expect(sameIgnoringIds({ id: "a", size: 1 }, { id: "a", size: 2 })).toBe(false);
    expect(sameIgnoringIds({ a: 1 }, { a: 1, b: 2 })).toBe(false);
    expect(sameIgnoringIds([1, 2], [2, 1])).toBe(false);
    expect(sameIgnoringIds([1], [1, 2])).toBe(false);
    expect(sameIgnoringIds({ a: null }, { a: undefined })).toBe(false);
    expect(sameIgnoringIds("x", "y")).toBe(false);
    expect(sameIgnoringIds(undefined, { id: "a" })).toBe(false);
  });
});

describe("keepIfSame", () => {
  it("returns the previous object when nothing but ids changed, so nothing downstream reacts", () => {
    const before = { id: "a", n: 1 };
    expect(keepIfSame({ id: "b", n: 1 }, before)).toBe(before);
    const changed = { id: "b", n: 2 };
    expect(keepIfSame(changed, before)).toBe(changed);
    expect(keepIfSame(changed, undefined)).toBe(changed);
  });
});

describe("reuseUnchanged", () => {
  const row = (id: string, key: string, n: number) => ({ id, key, n });

  it("keeps the previous object for every row that did not change and the previous id for every row that did", () => {
    const previous = [row("old_a", "a", 1), row("old_b", "b", 2)];
    const fresh = [row("new_a", "a", 1), row("new_b", "b", 3), row("new_c", "c", 4)];

    const result = reuseUnchanged(fresh, previous, (item) => item.key);

    expect(result[0]).toBe(previous[0]);
    expect(result[1]).toEqual({ id: "old_b", key: "b", n: 3 });
    expect(result[1]).not.toBe(previous[1]);
    expect(result[2]).toBe(fresh[2]);
  });

  it("leaves rows alone when a key is missing or repeated", () => {
    const previous = [row("old_1", "same", 1), row("old_2", "same", 1)];
    const fresh = [row("new_1", "same", 1)];

    expect(reuseUnchanged(fresh, previous, (item) => item.key)[0]).toBe(fresh[0]);
  });
});

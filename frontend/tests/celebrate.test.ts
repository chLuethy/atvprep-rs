import { describe, expect, it } from "vitest";
import { celebrateToday } from "../src/lib/celebrate";

const memory = () => {
  const data = new Map<string, string>();
  return { getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => void data.set(key, value) };
};

describe("celebrateToday", () => {
  it("says yes once a day for each moment, so it stays a pleasure", () => {
    const storage = memory();
    const morning = new Date("2026-10-24T08:00:00");

    expect(celebrateToday("all-ready", storage, morning)).toBe(true);
    expect(celebrateToday("all-ready", storage, new Date("2026-10-24T22:00:00"))).toBe(false);
    expect(celebrateToday("something-else", storage, morning)).toBe(true);
    expect(celebrateToday("all-ready", storage, new Date("2026-10-25T08:00:00"))).toBe(true);
  });

  it("never gets in the way when storage is unavailable", () => {
    const broken = { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } };

    expect(celebrateToday("all-ready", broken, new Date())).toBe(false);
  });
});

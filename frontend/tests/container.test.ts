import { describe, expect, it } from "vitest";
import { isMp4Like } from "../src/features/files/container";

describe("isMp4Like", () => {
  it("knows the containers that are verified and fixed in place", () => {
    expect([".mp4", ".M4V", ".mov"].every(isMp4Like)).toBe(true);
    expect([".mkv", ".avi", ".webm", ""].some(isMp4Like)).toBe(false);
  });
});

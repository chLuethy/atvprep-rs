import { describe, expect, it } from "vitest";
import { formatDateTime } from "../src/lib/time";

describe("formatDateTime", () => {
  it("returns the fallback for missing or unparseable timestamps", () => {
    expect(formatDateTime(null, "Unknown")).toBe("Unknown");
    expect(formatDateTime("", "Unknown")).toBe("Unknown");
    expect(formatDateTime("not a date", "Unknown")).toBe("Unknown");
    expect(formatDateTime("not a date")).toBe("");
  });

  it("formats a real timestamp", () => {
    expect(formatDateTime("2026-01-15T10:00:00Z")).not.toBe("");
    expect(formatDateTime("2026-01-15T10:00:00Z")).not.toContain("Invalid");
  });
});

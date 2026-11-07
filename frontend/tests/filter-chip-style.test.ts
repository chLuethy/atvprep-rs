import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync("src/styles/app.css", "utf8");

describe("filter chips", () => {
  it("show visibly which chips are selected", () => {
    expect(css).toMatch(/\.filter-chip\[aria-pressed="true"\]\s*\{[^}]*background/);
  });
  it("look like chips when not selected", () => {
    expect(css).toMatch(/\.filter-chip\s*\{[^}]*border/);
  });
});

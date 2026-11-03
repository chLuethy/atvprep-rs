import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const dir = join(__dirname, "../src/styles");
const files = readdirSync(dir).filter((name) => name.endsWith(".css"));
const all = files.map((name) => readFileSync(join(dir, name), "utf8")).join("\n");
const tokens = readFileSync(join(dir, "tokens.css"), "utf8");
const declarations = (property: string) => [...all.matchAll(new RegExp(`(?<![-\\w])${property}:\\s*([^;}]+)`, "g"))].map((match) => match[1]!.trim());
const ms = (name: string) => Number(new RegExp(`${name}:\\s*(\\d+)ms`).exec(tokens)![1]);

describe("motion", () => {
  it("has its timings and one easing as tokens, feedback short and nothing over half a second", () => {
    expect(tokens).toMatch(/--ease-out:\s*cubic-bezier\(0\.25,\s*1,\s*0\.5,\s*1\)/);
    expect(ms("--dur-fast")).toBeLessThanOrEqual(150);
    expect(ms("--dur-base")).toBeLessThanOrEqual(250);
    for (const name of ["--dur-fast", "--dur-base", "--dur-flash", "--dur-glide"]) expect(ms(name)).toBeLessThanOrEqual(800);
  });

  it("lets a progress bar glide steadily for as long as one read lasts, and nothing else", () => {
    expect(tokens).toMatch(/--ease-tick:\s*linear/);
    expect(ms("--dur-tick")).toBeGreaterThanOrEqual(1000);
    const users = [...all.matchAll(/([^{}]+)\{[^}]*var\(--dur-tick\)/g)].map((match) => match[1]!.trim());
    expect(users).toEqual([".bar-fill"]);
  });

  it("never writes a raw duration or curve into a rule", () => {
    const loose = [...declarations("transition"), ...declarations("animation")].filter((value) => /\d(ms|s)\b/.test(value.replace(/var\([^)]*\)/g, "")) || /ease|cubic-bezier|linear/.test(value.replace(/var\(--ease-[\w-]+\)/g, "")));
    expect(loose.filter((value) => value !== "none")).toEqual([]);
  });

  it("animates only compositor-friendly properties and colour, never layout", () => {
    const layout = /\b(width|height|margin[-\w]*|padding[-\w]*|top|left|right|bottom|inset|gap|flex[-\w]*)\b/;
    expect(declarations("transition").filter((value) => layout.test(value))).toEqual([]);
    expect(declarations("transition-property").filter((value) => layout.test(value))).toEqual([]);
  });

  it("defines the keyframes it uses", () => {
    for (const name of ["flash-fade", "rise", "fade-in"]) expect(all).toMatch(new RegExp(`@keyframes ${name}\\b`));
    for (const used of new Set([...all.matchAll(/animation:\s*([a-z-]+)\s/g)].map((match) => match[1]!).filter((name) => name !== "none"))) {
      expect(all).toMatch(new RegExp(`@keyframes ${used}\\b`));
    }
  });

  it("respects reduced motion and still shows a change without animating it", () => {
    expect(all).toMatch(/@media \(prefers-reduced-motion: reduce\)/);
    // The highlight itself is a plain background; only its fade is an animation.
    expect(all).toMatch(/\.flash\s*\{[^}]*background-color:\s*var\(--changed\)/);
  });

  it("loops only the breath of a running chip and the blink of the logo cursor while work runs, and keeps both states without motion", () => {
    const loops = [...all.matchAll(/([^{}]+)\{[^}]*animation:[^;]*infinite/g)].map((match) => match[1]!.trim());
    expect(loops.sort()).toEqual([".chip.live::before", ".logo.busy .logo-cursor"]);
    expect(ms("--dur-loop")).toBeGreaterThanOrEqual(2000);
    expect(all).toMatch(/\.chip\.live\s*\{[^}]*background:/); // the steady tint is there when animation is off
    expect(all).not.toMatch(/@keyframes pulse\b/);
  });

  it("has a tint for what just changed, in light and dark", () => {
    expect(tokens.match(/--changed:/g)?.length ?? 0).toBeGreaterThanOrEqual(3);
  });
});

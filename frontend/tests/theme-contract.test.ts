import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/* The contract of the theme scaffold (styles/theme.css): the one place that holds colour.
   Seeds are set once per variant; every other tone is derived; components only know semantic tokens. */

const dir = join(__dirname, "../src/styles");
const SCAFFOLD = "theme.css";
const strip = (text: string) => text.replace(/\/\*[\s\S]*?\*\//g, "");
const raw = (name: string) => readFileSync(join(dir, name), "utf8");
const css = (name: string) => strip(raw(name));
const files = readdirSync(dir).filter((name) => name.endsWith(".css"));
const components = files.filter((name) => name !== SCAFFOLD);

const SEEDS = ["accent", "ink", "ground", "surface", "line", "ok", "warn", "bad"].map((name) => `--seed-${name}`);

/** Custom properties declared in the scaffold: name -> value. */
const declared = (text: string) => new Map([...text.matchAll(/(?<![-\w])(--[\w-]+)\s*:\s*([^;]+);/g)].map((m) => [m[1]!, m[2]!.trim()]));
const scaffold = declared(css(SCAFFOLD));
const refs = (value: string) => [...value.matchAll(/var\((--[\w-]+)/g)].map((m) => m[1]!);

/** All seeds a token depends on, following var() references through the scaffold. */
const reach = (token: string, seen = new Set<string>()): Set<string> => {
  if (seen.has(token)) return seen;
  seen.add(token);
  for (const next of refs(scaffold.get(token) ?? "")) reach(next, seen);
  return seen;
};

describe("the theme scaffold", () => {
  it("is the only stylesheet with a colour literal", () => {
    // url(...) holds mask artwork whose colour never shows, so it is not a colour of the theme.
    const literal = /#[0-9a-fA-F]{3,8}(?![\w-])|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|(?<![-\w])(?:black|white)(?![-\w])/;
    const offenders = components.filter((name) => literal.test(css(name).replace(/url\([^)]*\)/g, "url()")));
    expect(offenders).toEqual([]);
  });

  it("sets each seed once, for light and dark together, so system and forced dark cannot drift", () => {
    for (const seed of SEEDS) {
      const all = [...css(SCAFFOLD).matchAll(new RegExp(`(?<![-\\w])${seed}\\s*:`, "g"))];
      expect(all, seed).toHaveLength(1);
      expect(scaffold.get(seed), seed).toMatch(/^light-dark\(\s*#[0-9a-f]{6}\s*,\s*#[0-9a-f]{6}\s*\)$/i);
    }
    // Nothing else in the scaffold holds a literal colour.
    const others = [...scaffold].filter(([name]) => !SEEDS.includes(name));
    for (const [name, value] of others) expect(value, name).not.toMatch(/#[0-9a-fA-F]{3,8}(?![\w-])/);
  });

  it("gives light, dark and system the same tokens: the variants only switch the colour scheme", () => {
    const text = css(SCAFFOLD);
    expect(text).toMatch(/:root\s*\{[^}]*color-scheme:\s*light dark/);
    expect(text).toMatch(/:root\[data-theme="light"\]\s*\{\s*color-scheme:\s*light;?\s*\}/);
    expect(text).toMatch(/:root\[data-theme="dark"\]\s*\{\s*color-scheme:\s*dark;?\s*\}/);
    expect(files.flatMap((name) => css(name).match(/prefers-color-scheme/g) ?? [])).toHaveLength(1); // tokens.css: leading only
  });

  it("derives every tone that is not a seed from seeds, by colour mixing or by naming one", () => {
    const derived = [...scaffold].filter(([name]) => !SEEDS.includes(name) && name.startsWith("--") && /^(?!--cursor)/.test(name));
    expect(derived.length).toBeGreaterThan(10);
    for (const [name, value] of derived) {
      expect(value, name).toMatch(/^var\(--[\w-]+\)$|color-mix\(in oklch,/);
      const seeds = [...reach(name)].filter((token) => SEEDS.includes(token));
      expect(seeds.length, `${name} reaches no seed`).toBeGreaterThan(0);
    }
  });

  it("lets the stylesheets use only semantic tokens, every one of which leads back to a seed", () => {
    const used = new Set(components.flatMap((name) => refs(css(name))));
    expect([...used].filter((name) => name.startsWith("--seed-"))).toEqual([]);
    const colourTokens = [...used].filter((name) => scaffold.has(name));
    expect(colourTokens.length).toBeGreaterThan(10);
    for (const name of colourTokens) expect([...reach(name)].some((token) => SEEDS.includes(token)), name).toBe(true);
  });

  it("defines no theme token outside the scaffold, so no second palette can start", () => {
    const names = new Set(scaffold.keys());
    const redefined = components.flatMap((name) => [...declared(css(name)).keys()].filter((token) => names.has(token)).map((token) => `${name}: ${token}`));
    expect(redefined).toEqual([]);
  });

  it("changes the whole app by swapping the accent seed: everything accent-coloured is derived from it", () => {
    // Swap proof: any token the components use for the accent reaches --seed-accent and no literal of its own.
    const accentTokens = ["--accent", "--accent-weak", "--on-accent"].filter((name) => scaffold.has(name));
    expect(accentTokens).toEqual(["--accent", "--accent-weak", "--on-accent"]);
    expect(reach("--accent").has("--seed-accent")).toBe(true);
    expect(reach("--accent-weak").has("--seed-accent")).toBe(true);
    // Only --seed-accent carries the accent's hex value.
    const hex = scaffold.get("--seed-accent")!.match(/#[0-9a-f]{6}/gi)!;
    const holders = [...scaffold].filter(([, value]) => hex.some((h) => value.toLowerCase().includes(h.toLowerCase()))).map(([name]) => name);
    expect(holders).toEqual(["--seed-accent"]);
  });

  it("explains every hand-set colour that is not a seed", () => {
    // Currently there are none; an exception needs a comment starting with "Exception:" right above it.
    const text = raw(SCAFFOLD);
    const handSet = [...text.matchAll(/(?<![-\w])(--(?!seed-)[\w-]+)\s*:\s*[^;]*#[0-9a-fA-F]{3,8}/g)].map((m) => m[1]!);
    for (const name of handSet) expect(text, name).toMatch(new RegExp(`Exception:[^*]*\\*/\\s*[^;]*${name}\\s*:`));
  });

  it("keeps the retired names out of the stylesheets and the logo", () => {
    const old = ["--bg", "--panel", "--side", "--line-strong", "--text", "--muted", "--faint", "--accent-ink", "--accent-strong", "--accent-text", "--ok-fill", "--warn-fill", "--bad-fill", "--brand-lime", "--brand-ink", "--cursor-edge"];
    const sources = [...files.map(css), readFileSync(join(__dirname, "../src/ui/Logo.tsx"), "utf8")].join("\n");
    const found = old.filter((name) => new RegExp(`var\\(${name}[,)]|(?<![-\\w])${name}\\s*:`).test(sources));
    expect(found).toEqual([]);
  });
});

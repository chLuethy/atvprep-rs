import { existsSync, readdirSync, readFileSync } from "node:fs";
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

const SEEDS = ["accent", "ink", "ground", "surface", "line", "muted", "sand", "on-accent", "ok", "warn", "bad"].map((name) => `--seed-${name}`);

/** Custom properties declared in the scaffold: name -> value. */
const declared = (text: string) => new Map([...text.matchAll(/(?<![-\w])(--[\w-]+)\s*:\s*([^;]+);/g)].map((m) => [m[1]!, m[2]!.trim()]));
const baseScaffold = /:root\s*\{([^}]*)\}/.exec(css(SCAFFOLD))![1]!;
const scaffold = declared(baseScaffold);
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
      const all = [...baseScaffold.matchAll(new RegExp(`(?<![-\\w])${seed}\\s*:`, "g"))];
      expect(all, seed).toHaveLength(1);
      expect(scaffold.get(seed), seed).toMatch(/^light-dark\(\s*#[0-9a-f]{6}\s*,\s*#[0-9a-f]{6}\s*\)$/i);
    }
    // Nothing else in the scaffold holds a literal colour.
    const others = [...scaffold].filter(([name]) => !SEEDS.includes(name));
    for (const [name, value] of others) expect(value, name).not.toMatch(/#[0-9a-fA-F]{3,8}(?![\w-])/);
  });

  it("enthält ausschließlich die Originalpalette ohne Naturpaletten oder Zweitakzente", () => {
    expect(css(SCAFFOLD)).not.toMatch(/data-palette|--seed-secondary|--accent-secondary/);
    expect(scaffold.get("--seed-accent")).toBe("light-dark(#805c3a, #c99d74)");
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

  it("shares the accent for actions and the mockup sand for selected surfaces", () => {
    // Swap proof: any token the components use for the accent reaches --seed-accent and no literal of its own.
    const accentTokens = ["--accent", "--accent-weak", "--on-accent"].filter((name) => scaffold.has(name));
    expect(accentTokens).toEqual(["--accent", "--accent-weak", "--on-accent"]);
    expect(reach("--accent").has("--seed-accent")).toBe(true);
    expect(reach("--accent-weak").has("--seed-sand")).toBe(true);
    expect(reach("--on-accent").has("--seed-on-accent")).toBe(true);
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

/* The building blocks of the design (#123): square, hairline, no shadow, no status fill. */

type Rule = { selector: string; body: string };
/** Innermost rules of a stylesheet, so rules inside @media and @keyframes are seen too. */
const rules = (name: string): Rule[] => [...css(name).matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({ selector: m[1]!.trim(), body: m[2]! }));
const allRules = components.flatMap((name) => rules(name).map((rule) => ({ ...rule, file: name })));

describe("the building blocks", () => {
  it("are square: every radius is 0", () => {
    const round = allRules.flatMap(({ file, selector, body }) =>
      [...body.matchAll(/(?<![-\w])[\w-]*radius\s*:\s*([^;]+)/g)].filter((m) => !/^0(px)?(\s+0(px)?)*$/.test(m[1]!.trim())).map((m) => `${file}: ${selector} -> ${m[0]}`));
    expect(round).toEqual([]);
  });

  it("have a shadow only on dialogs and the command palette; everything else is a hairline", () => {
    const OVERLAYS = new Set([".pal", ".help-panel"]);
    const shadowed = allRules.filter(({ body }) => /(?<![-\w])box-shadow\s*:/.test(body)).map(({ file, selector }) => `${file}: ${selector}`);
    expect(shadowed.filter((entry) => !OVERLAYS.has(entry.split(": ")[1]!))).toEqual([]);
    expect(shadowed.map((entry) => entry.split(": ")[1])).toEqual(expect.arrayContaining([".pal"]));
  });

  it("never show a status as a filled area: chips, badges and counts are text, symbol and hairline", () => {
    const filled = allRules
      .filter(({ selector }) => /\.(chip|nav-count)(?![-\w])/.test(selector) && !/filter-chip/.test(selector))
      .filter(({ body }) => [...body.matchAll(/(?<![-\w])background(?:-color)?\s*:\s*([^;]+)/g)].some((m) => !/^(none|transparent)$/.test(m[1]!.trim())))
      .map(({ file, selector }) => `${file}: ${selector}`);
    expect(filled).toEqual([]);
  });

  it("states the dashed column line once, as a token that two edge classes reuse", () => {
    const text = components.map(css).join("\n");
    expect(text.match(/linear-gradient\(var\(--accent\) 50%, transparent 0\)/g) ?? []).toHaveLength(1);
    expect(css("tokens.css")).toMatch(/--guide:\s*linear-gradient\(var\(--accent\) 50%, transparent 0\)/);
    const edges = allRules.filter(({ body }) => /var\(--guide\)/.test(body)).map(({ selector }) => selector);
    expect(edges).toEqual(expect.arrayContaining([".guide-l", ".guide-r", ".sidebar::after"]));
  });

  it("draws the label style once: accent colour and a colon", () => {
    const label = allRules.find(({ selector }) => /\.label(?![-\w])/.test(selector) && /color:\s*var\(--accent\)/.test(selector + allRules[0]!.body) || /\.label/.test(selector));
    expect(label, "a shared label rule").toBeDefined();
    const colon = allRules.find(({ selector, body }) => /\.label/.test(selector) && selector.endsWith("::after") && /content:\s*":"/.test(body));
    expect(colon, "the colon").toBeDefined();
    expect(allRules.find(({ selector, body }) => /\.label/.test(selector) && /color:\s*var\(--accent\)/.test(body))).toBeDefined();
  });

  it("puts the arrow of a select at its right edge, without the browser's own arrow", () => {
    const select = allRules.find(({ selector }) => selector === ".select")!;
    expect(select.body).toMatch(/appearance:\s*none/);
    expect(select.body).toMatch(/background-image:[^;]*linear-gradient/);
    expect(select.body).toMatch(/background-position:\s*calc\(100% -/);
  });

  it("draws the logo from theme tokens, square, with no colour of its own", () => {
    const logo = readFileSync(join(__dirname, "../src/ui/Logo.tsx"), "utf8");
    expect(logo).not.toMatch(/#[0-9a-fA-F]{3,8}\b|\brx=|rgb\(/);
    expect(logo).toMatch(/var\(--accent\)/);
    expect(logo).toMatch(/var\(--on-accent\)/);
    expect(css("app.css")).toMatch(/\.logo\.busy \.logo-cursor[^{]*\{[^}]*animation:\s*blink/); // the cursor still lights up at work
  });
});

describe("the fonts", () => {
  const fontDir = join(__dirname, "../src/assets/fonts");
  const faces = [...css("fonts.css").matchAll(/font-family:\s*"([^"]+)"[^}]*url\("([^"]+)"\)/g)].map((m) => ({ family: m[1]!, url: m[2]! }));

  it("set text in the Helvetica system stack and technical values in JetBrains Mono", () => {
    expect(declared(css("tokens.css")).get("--font-ui")).toMatch(/^"Helvetica Neue", Helvetica, Arial/);
    expect(declared(css("tokens.css")).get("--font-mono")).toMatch(/^"JetBrains Mono"/);
  });

  it("ship with the app, so the manager looks the same offline: every font file is there, with its licence", () => {
    expect(faces.length).toBeGreaterThan(0);
    for (const { url } of faces) expect(existsSync(join(dir, url)), url).toBe(true);
    expect(existsSync(join(fontDir, "OFL-JetBrains-Mono.txt"))).toBe(true);
    expect(new Set(faces.map(({ family }) => family))).toEqual(new Set(["JetBrains Mono"]));
  });

  it("no longer carry IBM Plex Sans, neither as a face, a licence nor a file", () => {
    expect(readdirSync(fontDir).filter((name) => /plex/i.test(name))).toEqual([]);
    expect(components.filter((name) => /plex/i.test(raw(name)))).toEqual([]);
  });
});

describe("the single-column breakpoint", () => {
  const constant = Number(/SINGLE_COLUMN_MAX\s*=\s*(\d+)/.exec(readFileSync(join(__dirname, "../src/lib/fitBottom.ts"), "utf8"))![1]);

  it("is one number: every CSS media query of the single column repeats the constant", () => {
    const literals = components.flatMap((name) => [...css(name).matchAll(/@media\s*\(max-width:\s*(\d+)px\)/g)].map((m) => ({ name, width: Number(m[1]) })));
    const near = literals.filter(({ width }) => Math.abs(width - constant) < 40 && width !== constant);
    expect(near).toEqual([]);
    expect(literals.filter(({ width }) => width === constant).length).toBeGreaterThanOrEqual(3);
  });

  it("is used by the code instead of a literal of its own", () => {
    const activity = readFileSync(join(__dirname, "../src/features/activity/ActivityPage.tsx"), "utf8");
    expect(activity).toContain("SINGLE_COLUMN_MAX");
    expect(activity).not.toMatch(/max-width:\s*\d+px/);
  });
});

describe("the favicon", () => {
  it("is the one file outside the scaffold with colour literals, and uses only light seed values", () => {
    // A favicon cannot read CSS variables, so its hex values repeat the light side of the seeds (public/favicon.svg).
    const svg = readFileSync(join(__dirname, "../public/favicon.svg"), "utf8");
    const used = [...new Set([...svg.matchAll(/#[0-9a-fA-F]{6}/g)].map((m) => m[0].toLowerCase()))];
    const light = SEEDS.flatMap((seed) => [...(scaffold.get(seed) ?? "").matchAll(/light-dark\(\s*(#[0-9a-f]{6})/gi)].map((m) => m[1]!.toLowerCase()));
    expect(used.length).toBeGreaterThan(0);
    expect(used.filter((hex) => !light.includes(hex))).toEqual([]);
  });
});

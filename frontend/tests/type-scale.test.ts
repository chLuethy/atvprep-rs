import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const dir = join(__dirname, "../src/styles");
const files = readdirSync(dir).filter((name) => name.endsWith(".css"));
const css = (name: string) => readFileSync(join(dir, name), "utf8");
const declarations = (property: string) =>
  files.flatMap((name) => [...css(name).matchAll(new RegExp(`(?<![-\\w])${property}:\\s*([^;}]+)`, "g"))].map((match) => ({ file: name, value: match[1]!.trim() })));

const SIZES = ["--text-xs", "--text-sm", "--text-base", "--text-md", "--text-lg"];

describe("the type scale", () => {
  it("defines five sizes in rem, so the browser's own font setting still applies", () => {
    const tokens = css("tokens.css");
    for (const name of SIZES) expect(tokens).toMatch(new RegExp(`${name}:\\s*[0-9.]+rem;`));
    const values = SIZES.map((name) => Number(new RegExp(`${name}:\\s*([0-9.]+)rem`).exec(tokens)![1]));
    expect([...values].sort((a, b) => a - b)).toEqual(values);
  });

  it("is the only source of font sizes: no px, no loose value", () => {
    const loose = declarations("font-size").filter(({ value }) => !SIZES.some((name) => value === `var(${name})`) && value !== "inherit");
    expect(loose).toEqual([]);
  });

  it("sizes the page from the base token, not a pixel value", () => {
    const body = /^body\s*\{[^}]*\}/m.exec(css("app.css"))![0];
    expect(body).toMatch(/font-size:\s*var\(--text-base\)/);
    expect(body).toMatch(/line-height:\s*var\(--leading-body\)/);
  });

  it("uses one tracking for capital labels and one for titles", () => {
    const allowed = new Set(["var(--tracking-caps)", "var(--tracking-title)", "normal", "0"]);
    expect(declarations("letter-spacing").filter(({ value }) => !allowed.has(value))).toEqual([]);
  });

  it("states the capital-label style once, however many headings use it", () => {
    const withCaps = files.flatMap((name) => [...css(name).matchAll(/text-transform:\s*uppercase/g)].map(() => name));
    expect(withCaps).toHaveLength(1);
  });

  it("keeps titles tight, wraps them evenly and limits the line length of prose", () => {
    const text = files.map(css).join("\n");
    expect(text).toMatch(/text-wrap:\s*balance/);
    expect(text).toMatch(/text-wrap:\s*pretty/);
    expect(text).toMatch(/max-width:\s*(6[0-9]|7[0-5])ch/);
    expect(text).toMatch(/--leading-title/);
  });

  it("sets titles apart by weight as well as size", () => {
    const base = /^h1, h2\s*\{[^}]*\}/m.exec(css("app.css"))![0];
    expect(base).toMatch(/font-weight:\s*600/);
  });

  it("lifts the leading for light text on a dark ground", () => {
    const tokens = css("tokens.css");
    const dark = tokens.slice(tokens.indexOf("prefers-color-scheme: dark"));
    expect(dark).toMatch(/--leading-body:\s*1\.5/);
  });
});

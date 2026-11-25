import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const dir = join(__dirname, "../src/styles");
const files = readdirSync(dir).filter((name) => name.endsWith(".css"));
const css = (name: string) => readFileSync(join(dir, name), "utf8");
const declarations = (property: string) =>
  files.flatMap((name) => [...css(name).matchAll(new RegExp(`(?<![-\\w])${property}:\\s*([^;}]+)`, "g"))].map((match) => ({ file: name, value: match[1]!.trim() })));

const SIZES = ["--text-xs", "--text-sm", "--text-code", "--text-control", "--text-base", "--text-md", "--text-lg"];

describe("the type scale", () => {
  it("defines the mockup sizes in rem, so the browser's own font setting still applies", () => {
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

  it("states the label style once, however many headings use it, and sets no capitals", () => {
    const labels = files.flatMap((name) => [...css(name).matchAll(/\{[^{}]*letter-spacing:\s*var\(--tracking-caps\)[^{}]*\}/g)].map(() => name));
    expect(labels).toHaveLength(1);
    expect(files.flatMap((name) => css(name).match(/text-transform:\s*uppercase/g) ?? [])).toHaveLength(0);
  });

  it("keeps titles tight, wraps them evenly and limits the line length of prose", () => {
    const text = files.map(css).join("\n");
    expect(text).toMatch(/text-wrap:\s*balance/);
    expect(text).toMatch(/text-wrap:\s*pretty/);
    expect(text).toMatch(/max-width:\s*(6[0-9]|7[0-5])ch/);
    expect(text).toMatch(/--leading-title/);
  });

  it("uses regular headings and the mockup's 15px body and 20px title bar", () => {
    const base = /^h1, h2\s*\{[^}]*\}/m.exec(css("app.css"))![0];
    expect(base).toMatch(/font-weight:\s*400/);
    expect(css("tokens.css")).toMatch(/--text-base:\s*0\.9375rem/);
    expect(css("tokens.css")).toMatch(/--text-md:\s*1\.25rem/);
    expect(css("app.css")).toMatch(/\.page-titlebar\s*\{[^}]*font-size:\s*var\(--text-md\)/);
  });

  it("lifts the leading for light text on a dark ground", () => {
    const tokens = css("tokens.css");
    const dark = tokens.slice(tokens.indexOf("prefers-color-scheme: dark"));
    expect(dark).toMatch(/--leading-body:\s*1\.5/);
  });
});

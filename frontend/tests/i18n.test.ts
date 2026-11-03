import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { language, N, setLanguage, t, tn } from "../src/lib/i18n";
import { de } from "../src/locales/de";

afterEach(() => {
  setLanguage("en");
  window.localStorage.clear();
});

describe("t and tn", () => {
  it("returns the English text as written, filled with its values", () => {
    expect(t("Anything at all")).toBe("Anything at all");
    expect(t("Hello {name}", { name: "Wave" })).toBe("Hello Wave");
    expect(tn("{count} file", "{count} files", 1)).toBe("1 file");
    expect(tn("{count} file", "{count} files", 3)).toBe("3 files");
    expect(N("Kept in a constant")).toBe("Kept in a constant");
  });

  it("looks German up in the catalogue and falls back to English for a text without an entry", () => {
    de["__probe"] = "Probe";
    de["__probe {n}"] = "Probe {n}";
    de["{count} __probe"] = ["{count} Probe", "{count} Proben"];
    try {
      setLanguage("de");
      expect(language()).toBe("de");
      expect(t("__probe")).toBe("Probe");
      expect(t("__probe {n}", { n: 2 })).toBe("Probe 2");
      expect(tn("{count} __probe", "{count} __probes", 1)).toBe("1 Probe");
      expect(tn("{count} __probe", "{count} __probes", 5)).toBe("5 Proben");
      expect(t("Not in the catalogue")).toBe("Not in the catalogue");
      expect(document.documentElement.lang).toBe("de");
    } finally {
      delete de["__probe"];
      delete de["__probe {n}"];
      delete de["{count} __probe"];
    }
  });

  it("remembers the choice", () => {
    setLanguage("de");
    expect(window.localStorage.getItem("atvprep.language")).toBe("de");
  });
});

const files = (dir: string): string[] =>
  readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : /\.tsx?$/.test(name) && !path.includes("locales") && !path.endsWith("lib/i18n.ts") ? [path] : [];
  });

const LITERAL = String.raw`"((?:[^"\\\n]|\\.)*)"`;
const unescape = (text: string) => text.replace(/\\(.)/g, "$1");

/** Every text the code asks to have translated: t("..."), N("..."), and the first form of tn("...", "..."). */
function used(): Map<string, { file: string; plural: boolean }> {
  const found = new Map<string, { file: string; plural: boolean }>();
  for (const file of files("src")) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(new RegExp(String.raw`(?<![\w.])(t|N)\(\s*${LITERAL}`, "g"))) found.set(unescape(match[2] ?? ""), { file, plural: false });
    for (const match of source.matchAll(new RegExp(String.raw`(?<![\w.])tn\(\s*${LITERAL}\s*,\s*${LITERAL}`, "g"))) found.set(unescape(match[1] ?? ""), { file, plural: true });
  }
  return found;
}

describe("the German catalogue", () => {
  it("has a German text for every text the code translates", () => {
    const missing = [...used()].filter(([text]) => !(text in de)).map(([text, where]) => `${where.file}: ${text}`);
    expect(missing).toEqual([]);
  });

  it("gives a plural pair to every counted text, and plain text to every other", () => {
    const wrong = [...used()].filter(([text, where]) => text in de && Array.isArray(de[text]) !== where.plural).map(([text]) => text);
    expect(wrong).toEqual([]);
  });

  it("keeps the {values} of the English text in the German one", () => {
    const names = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
    const broken = Object.entries(de).filter(([english, german]) => {
      const forms = typeof german === "string" ? [german] : [...german];
      const wanted = names(english).filter((name) => name !== "count");
      return forms.some((form) => names(form).filter((name) => name !== "count").join() !== wanted.join());
    }).map(([english]) => english);
    expect(broken).toEqual([]);
  });

  it("has no entry that nothing uses", () => {
    const all = used();
    const unused = Object.keys(de).filter((text) => !all.has(text));
    expect(unused).toEqual([]);
  });
});

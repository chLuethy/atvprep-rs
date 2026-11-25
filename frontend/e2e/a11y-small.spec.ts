import { createRequire } from "node:module";
import { expect, test, type Page } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

const AXE_SOURCE = createRequire(import.meta.url).resolve("axe-core/axe.min.js");

/** The ids of the axe rules that fail on the current page. */
async function failedRules(page: Page, rules: string[]): Promise<string[]> {
  await page.addScriptTag({ path: AXE_SOURCE });
  return page.evaluate(async (values) => {
    const axe = (window as unknown as { axe: { run: (ctx: Document, opts: object) => Promise<{ violations: Array<{ id: string }> }> } }).axe;
    const results = await axe.run(document, { runOnly: { type: "rule", values } });
    return results.violations.map((violation) => violation.id);
  }, rules);
}

test.describe("Barrierefreiheit und Kleinkram", () => {
  test("die Einstellungen haben ein h1, der Menü-Button keine Namensabweichung", async ({ page }) => {
    await openAndUnlock(page, "/settings");
    await page.setViewportSize({ width: 390, height: 800 });
    await expect(page.getByRole("heading", { level: 1, name: "Runtime settings" })).toBeVisible();

    expect(await failedRules(page, ["page-has-heading-one", "label-content-name-mismatch"])).toEqual([]);
  });

  test("Tab durchläuft die Dateiliste mit einem Stopp je Zeile", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await expect(page.getByRole("link", { name: "Sample Movie" })).toBeVisible();
    const links = page.locator("a.file-title");
    for (let index = 0; index < (await links.count()); index += 1) {
      await expect(links.nth(index)).toHaveAttribute("tabindex", "-1");
    }
    await page.locator("tr.row[tabindex='0']").focus();
    await page.keyboard.press("Tab");
    expect(await page.evaluate(() => document.activeElement?.closest("table.files-table") ?? null)).toBeNull();
  });

  test("das Palettenfeld zeigt einen sichtbaren Fokus", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await page.keyboard.press("ControlOrMeta+k");
    const input = page.getByRole("combobox", { name: "Search commands" });
    await expect(input).toBeFocused();

    const outline = await input.evaluate((element) => {
      const style = getComputedStyle(element);
      return { style: style.outlineStyle, width: parseFloat(style.outlineWidth) };
    });
    expect(outline.style).not.toBe("none");
    expect(outline.width).toBeGreaterThanOrEqual(2);
  });

  test("der Seitentitel nennt die Route", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await expect(page).toHaveTitle(/Files · atvprep-manager/);
    await page.getByRole("link", { name: "Settings" }).click();
    await expect(page).toHaveTitle(/Settings · atvprep-manager/);
  });

  test("bei einer unbekannten Datei-ID erscheint keine Auswahlleiste", async ({ page }) => {
    await openAndUnlock(page, "/files/m_gibt_es_nicht");
    await expect(page.getByText("Unable to load file detail")).toBeVisible();
    await expect(page.getByRole("region", { name: "Selection" })).toHaveCount(0);
  });

  test("Einstellungstexte überschreiten 90 Zeichen je Zeile nicht", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openAndUnlock(page, "/settings");
    await expect(page.getByLabel("Scan interval (seconds)", { exact: true })).toBeVisible();

    const widest = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>(".settings-body p, .settings-body .help, .appearance-settings .help")]
        .filter((element) => (element.textContent ?? "").length > 90)
        .map((element) => {
          // A probe one `ch` wide inside the element has the element's own font.
          const probe = document.createElement("span");
          probe.style.cssText = "display:inline-block;width:1ch;height:0";
          element.append(probe);
          const chWidth = probe.getBoundingClientRect().width;
          probe.remove();
          return Math.round(element.getBoundingClientRect().width / chWidth);
        }),
    );
    expect(widest.length).toBeGreaterThan(0);
    for (const chars of widest) expect(chars).toBeLessThanOrEqual(90);
  });
});

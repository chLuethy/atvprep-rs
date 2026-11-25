import { expect, test, type Locator, type Page } from "@playwright/test";
import { openThemed, type Theme } from "./design";

test.use({ trace: "off", screenshot: "off", video: "off" });

/* "Why it failed" of a job on a Sonarr-named file: chip and name side by side, the error in plain words. */

const box = async (target: Locator) => {
  const found = await target.boundingBox();
  if (!found) throw new Error("element has no box");
  return found;
};
const overlap = (a: { x: number; y: number; width: number; height: number }, b: typeof a) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

async function openLongFailure(page: Page, theme: Theme, width = 1440): Promise<Locator> {
  // Unlock wide (the Lock button hides in the narrow menu), then measure at the width asked for.
  await page.setViewportSize({ width: 1440, height: 900 });
  await openThemed(page, "/jobs", theme, "Activity");
  await page.setViewportSize({ width, height: 900 });
  await page.getByRole("region", { name: "History" }).getByRole("link", { name: /Ya Shivu/ }).first().click();
  const why = page.getByRole("region", { name: "Why it failed" });
  await expect(why).toBeVisible();
  return why;
}

test.describe("job failure detail", () => {
  for (const theme of ["light", "dark"] as const) {
    for (const width of [1440, 390]) {
      test(`chip and file name do not overlap (${theme}, ${width} px)`, async ({ page }) => {
        const why = await openLongFailure(page, theme, width);
        const item = why.getByRole("listitem").first();
        const chip = item.locator(".chip").first();
        const name = item.locator("b");
        await expect(name).toContainText("Ya Shivu");
        const chipBox = await box(chip);
        const chipText = await box(chip.locator(".chip-text"));
        const nameBox = await box(name);
        // The chip is as wide as its words, and neither it nor its words touch the name.
        expect(chipText.x + chipText.width).toBeLessThanOrEqual(chipBox.x + chipBox.width + 1);
        expect(overlap(chipBox, nameBox)).toBe(false);
        expect(overlap(chipText, nameBox)).toBe(false);
        // Nothing sticks out of the section sideways.
        const sectionBox = await box(why);
        expect(nameBox.x + nameBox.width).toBeLessThanOrEqual(sectionBox.x + sectionBox.width + 1);
      });
    }
  }

  test("a known error code reads as a sentence and the raw code only on request", async ({ page }) => {
    const why = await openLongFailure(page, "light");
    await expect(why).toContainText("The worker stopped while this ran");
    await expect(why.getByText("worker_stopped")).toBeHidden();
    await why.getByText("Show code").click();
    await expect(why.getByText("worker_stopped")).toBeVisible();
  });

  test("the German interface shows the known error in German and an unknown one as written", async ({ page }) => {
    await openThemed(page, "/jobs", "light");
    await page.evaluate(() => window.localStorage.setItem("atvprep.language", "de"));
    await page.reload();
    await page.getByRole("region", { name: "Verlauf" }).getByRole("link", { name: /Ya Shivu/ }).first().click();
    const why = page.getByRole("region", { name: "Warum es fehlschlug" });
    await expect(why).toContainText("Der Worker wurde beendet, während dieser Auftrag lief");
    await expect(why).not.toContainText("The worker stopped");
    await expect(why.getByText("worker_stopped")).toBeHidden();
  });
});

import { expect, test, type Locator, type Page } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

/* Phone layout (spec #151, ticket #156), measured in a real browser: touch targets, filter bar, inspector. */

const MIN_TARGET = 24; // WCAG 2.5.8 (AA)

/** Every element of `target` that is smaller than 24x24 CSS px, as readable lines. */
async function undersized(target: Locator): Promise<string[]> {
  return target.evaluateAll((all, min) => all.flatMap((element) => {
    const { width, height } = element.getBoundingClientRect();
    return width + 0.5 < min || height + 0.5 < min
      ? [`${element.tagName.toLowerCase()} "${(element.textContent ?? "").trim().slice(0, 30)}" ${width.toFixed(1)}x${height.toFixed(1)}`]
      : [];
  }), MIN_TARGET);
}

const phone = async (page: Page, path: string) => {
  // The Lock button sits in the menu on a phone: unlock on a wide window, then narrow it.
  await page.setViewportSize({ width: 1440, height: 900 });
  await openAndUnlock(page, path);
  await page.setViewportSize({ width: 390, height: 800 });
};

test.describe("phone, 390 px", () => {
  test("sort buttons and file title links are at least 24 px", async ({ page }) => {
    await phone(page, "/files");
    await expect(page.locator("a.file-title").first()).toBeVisible();
    const sorts = page.locator("button.th-btn");
    expect(await sorts.count()).toBeGreaterThan(0);
    expect(await undersized(sorts)).toEqual([]);
    expect(await undersized(page.locator("a.file-title"))).toEqual([]);
  });

  test("the links of the overview are at least 24 px", async ({ page }) => {
    await phone(page, "/dashboard");
    const links = page.locator(".overview li a, .overview a.health-row");
    await expect(links.first()).toBeVisible();
    expect(await undersized(links)).toEqual([]);
  });

  test("the check boxes of the settings are at least 24 px", async ({ page }) => {
    await phone(page, "/settings");
    const boxes = page.getByRole("checkbox");
    await expect(boxes.first()).toBeVisible();
    expect(await undersized(boxes)).toEqual([]);
  });

  test("More filters is always whole in the filter bar, and the page does not scroll sideways", async ({ page }) => {
    await phone(page, "/files");
    const bar = page.getByRole("search", { name: "Filter files" });
    const more = bar.getByRole("button", { name: "More filters" });
    await expect(more).toBeVisible();
    const inside = async () => {
      const row = (await bar.locator(".fb-facets").boundingBox())!;
      const box = (await more.boundingBox())!;
      return box.x >= row.x - 0.5 && box.x + box.width <= row.x + row.width + 0.5;
    };
    expect(await inside()).toBe(true);
    // Scrolled to the far end the toggle is still whole: it does not scroll away.
    await bar.locator(".fb-facets").evaluate((row) => { row.scrollLeft = row.scrollWidth; });
    expect(await inside()).toBe(true);
    expect(await undersized(more)).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });

  test("an opened inspector comes into view and is announced", async ({ page }) => {
    await phone(page, "/files");
    await page.getByRole("link", { name: "Failed Movie" }).click();
    const detail = page.getByRole("complementary", { name: "Details" });
    await expect(detail.getByRole("heading", { level: 2 }).first()).toBeInViewport();
    await expect(page.locator("[aria-live='polite']").filter({ hasText: "Failed Movie" })).toBeVisible();
  });
});

test("on a desktop the inspector opens without scrolling the page or the list", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openAndUnlock(page, "/files");
  await expect(page.getByRole("link", { name: "Sample Movie" })).toBeVisible();
  const scrolled = () => page.evaluate(() => ({ page: window.scrollY, list: document.querySelector(".files-list > .lib-scroll")?.scrollTop ?? 0 }));
  const before = await scrolled();
  // A raw mouse click: Playwright's own click would scroll the row into view and blur the measurement.
  const box = await page.locator("tr.row").first().boundingBox();
  await page.mouse.click(box!.x + box!.width - 10, box!.y + box!.height / 2);
  await expect(page.getByRole("complementary", { name: "Details" })).toBeVisible();
  await page.waitForTimeout(300);
  expect(await scrolled()).toEqual(before);
});

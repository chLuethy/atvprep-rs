import { expect, test } from "@playwright/test";
import { contrastViolations, expectDesignHolds, narrowOverflow, openThemed } from "./design";

test.use({ trace: "off", screenshot: "off", video: "off" });

// Pages under the design check. Page tickets append their route here.
const PAGES: ReadonlyArray<{ name: string; path: string; ready: string }> = [
  { name: "Overview", path: "/dashboard", ready: "Overview" },
];

for (const { name, path, ready } of PAGES) {
  test(`${name}: contrast in light and dark, no horizontal scroll at 560 px`, async ({ page }) => {
    await expectDesignHolds(page, path, { heading: ready });
  });
}

// The helper itself must be able to fail: each case plants a violation in the page.
test.describe("the design helper reports violations", () => {
  test("light grey text on white fails the contrast check", async ({ page }) => {
    await openThemed(page, "/dashboard", "light");
    await page.evaluate(() => {
      const el = document.createElement("p");
      el.id = "planted-contrast";
      el.textContent = "Planted low contrast";
      el.style.cssText = "color:#eeeeee;background:#ffffff;position:fixed;top:0;left:0;z-index:9999;margin:0";
      document.body.append(el);
    });
    const found = await contrastViolations(page);
    expect(found.join("\n")).toContain("planted-contrast");
  });

  test("a clean page has no contrast violations", async ({ page }) => {
    await openThemed(page, "/dashboard", "dark");
    expect(await contrastViolations(page)).toEqual([]);
  });

  test("an over-wide element is reported at 560 px", async ({ page }) => {
    await openThemed(page, "/dashboard", "light");
    await page.evaluate(() => {
      const el = document.createElement("div");
      el.style.cssText = "width:900px;height:4px";
      document.body.append(el);
    });
    expect(await narrowOverflow(page)).toBeGreaterThan(0);
  });
});

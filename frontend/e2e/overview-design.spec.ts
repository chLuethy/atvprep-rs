import { expect, test, type Locator, type Page } from "@playwright/test";
import { openThemed } from "./design";

test.use({ trace: "off", screenshot: "off", video: "off" });

const box = async (target: Locator) => {
  const found = await target.boundingBox();
  if (!found) throw new Error("element has no box");
  return found;
};
const region = (page: Page, name: string) => page.getByRole("region", { name, exact: true });

test.describe("overview layout (design reference: docs/design/manager-mockup.html, #uebersicht)", () => {
  test("two independent columns without a gap under short panels", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openThemed(page, "/dashboard", "light", "Overview");
    // The first panel of the left column: leftovers when a scan found some (another journey removes them), else needs.
    const first = await box(page.locator(".ov-col").first().locator(".ov-sec").first());
    const needs = await box(region(page, "Needs you"));
    const running = await box(region(page, "Running now"));
    const health = await box(region(page, "File health"));

    // Left column: panels stack directly; right column: health starts level with the left column.
    expect(Math.abs(health.y - first.y)).toBeLessThan(2);
    expect(health.x).toBeGreaterThan(needs.x + needs.width - 1);
    expect(Math.abs(running.x - needs.x)).toBeLessThan(2);
    // The gap between two stacked panels is one panel gap, however tall the neighbouring column is.
    const gap = running.y - (needs.y + needs.height);
    expect(gap).toBeGreaterThanOrEqual(8);
    expect(gap).toBeLessThanOrEqual(24);
  });

  test("a single column below 1180 px", async ({ page }) => {
    await page.setViewportSize({ width: 1100, height: 900 });
    await openThemed(page, "/dashboard", "light", "Overview");
    const needs = await box(region(page, "Needs you"));
    const health = await box(region(page, "File health"));
    expect(Math.abs(health.x - needs.x)).toBeLessThan(2);
    expect(health.y).toBeGreaterThan(needs.y + needs.height - 1);
  });

  for (const theme of ["light", "dark"] as const) {
    test(`file health rows: badges left-aligned, same spacing to the bar, bars 20 px (${theme})`, async ({ page }) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await openThemed(page, "/dashboard", theme, "Overview");
      const rows = region(page, "File health").locator("[data-health-row]");
      await rows.first().waitFor();
      const count = await rows.count();
      expect(count).toBeGreaterThanOrEqual(2);
      const measured: Array<{ badgeX: number; badgeH: number; barX: number; barH: number; gap: number }> = [];
      for (let i = 0; i < count; i++) {
        const badge = await box(rows.nth(i).locator(".chip"));
        const bar = await box(rows.nth(i).locator("[data-health-bar]"));
        measured.push({ badgeX: badge.x, badgeH: badge.height, barX: bar.x, barH: bar.height, gap: bar.x - (badge.x + badge.width) });
      }
      for (const row of measured) {
        expect(row.badgeX).toBeCloseTo(measured[0]!.badgeX, 0);
        expect(row.barX).toBeCloseTo(measured[0]!.barX, 0);
        expect(row.barH).toBe(20);
        expect(row.badgeH).toBe(20);
        // The label column is as wide as the longest label, so no badge touches or overlaps the bar.
        expect(row.gap).toBeGreaterThanOrEqual(8);
      }
    });
  }
});

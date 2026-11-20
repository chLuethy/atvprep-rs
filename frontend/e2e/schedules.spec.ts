import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("schedules journey", () => {
  test("a schedule that prepares starts paused at Copy only, and turning it on names what is signed", async ({ page }) => {
    await openAndUnlock(page, "/schedules");
    await page.getByRole("button", { name: /New schedule/ }).click();
    const editor = page.getByRole("region", { name: "New schedule" });
    await editor.getByLabel("Name").fill("Nightly prepare");
    await editor.getByRole("group", { name: "Action" }).getByRole("button", { name: "Prepare" }).click();
    await expect(editor.getByRole("radio", { name: "Copy only" })).toBeChecked();
    await editor.getByRole("radio", { name: "Encode audio", exact: true }).check();
    await editor.getByLabel("Files per run").fill("5");
    await editor.getByRole("button", { name: "Save schedule" }).click();
    await expect(page.getByText("Schedule saved. It is paused until you turn it on.")).toBeVisible();

    const list = page.getByRole("list", { name: "Schedules" });
    await expect(list).toContainText("All libraries · up to 5 files · Encode audio");
    await page.getByRole("button", { name: "Turn on Nightly prepare" }).click();
    const question = page.getByRole("region", { name: "Turn on Nightly prepare" });
    await expect(question).toContainText("Encoding allowance: Encode audio.");
    await expect(question).toContainText("Target profile: CRF 18, HEVC10 off, keep AV1 off, DTS fix off.");
    await expect(question).toContainText(/files? match(es)? now/);
    await question.getByRole("button", { name: "Confirm turn on" }).click();
    await expect(page.getByText("Schedule is on.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Pause Nightly prepare" })).toBeVisible();
  });

  test("every card has the same columns, with dashed lines of equal length, and the actions sit in a band of their own", async ({ page }) => {
    await openAndUnlock(page, "/schedules");
    // Cards of different content: the seeded verify one, a scan one and a prepare one with a long name.
    for (const [name, action] of [["Scan everything", "Scan"], ["Prepare the long running series of the winter", "Prepare"]] as const) {
      await page.getByRole("button", { name: /New schedule/ }).click();
      const editor = page.getByRole("region", { name: "New schedule" });
      await editor.getByLabel("Name").fill(name);
      await editor.getByRole("group", { name: "Action" }).getByRole("button", { name: action }).click();
      if (action === "Prepare") await editor.getByLabel("Files per run").fill("5");
      await editor.getByRole("button", { name: "Save schedule" }).click();
      await expect(page.getByRole("article", { name })).toBeVisible();
    }
    const cards = page.getByRole("article");
    // The journey above may have left its own schedule behind: at least the seeded one and the two new ones.
    await expect.poll(() => cards.count()).toBeGreaterThanOrEqual(3);

    const measure = () => cards.evaluateAll((nodes) => nodes.map((card) => {
      const columns = Array.from(card.querySelectorAll(":scope > .sched > div")).map((column) => {
        const box = column.getBoundingClientRect();
        return { left: Math.round(box.left), width: Math.round(box.width), height: Math.round(box.height), guide: getComputedStyle(column).backgroundImage };
      });
      const band = card.querySelector(":scope > .sched-actions") as HTMLElement;
      return { columns, band: { top: getComputedStyle(band).borderTopWidth } };
    }));

    await page.setViewportSize({ width: 1440, height: 900 });
    const wide = await measure();
    expect(wide[0]?.columns).toHaveLength(5);
    for (const card of wide) {
      // Same left edge and width of every column in every card.
      expect(card.columns.map((c) => [c.left, c.width])).toEqual(wide[0]?.columns.map((c) => [c.left, c.width]));
      // The dashed line stands at the left of columns two to five, and every column is as tall as its card's row.
      expect(card.columns.slice(1).every((c) => c.guide.includes("linear-gradient"))).toBe(true);
      expect(new Set(card.columns.map((c) => c.height)).size).toBe(1);
      expect(card.band.top).toBe("1px");
    }

    // The band keeps its two groups apart: frequent actions left, rare ones right.
    const main = await cards.first().locator(".sched-actions-main").boundingBox();
    const end = await cards.first().locator(".sched-actions-end").boundingBox();
    expect(main && end && main.x + main.width <= end.x).toBe(true);

    // Narrow: nothing is wider than the window, and the band wraps instead of widening the page.
    await page.setViewportSize({ width: 560, height: 900 });
    for (const card of await measure()) {
      expect(card.columns.every((c) => c.left + c.width <= 560)).toBe(true);
    }
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
  });
});

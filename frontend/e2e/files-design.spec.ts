import { expect, test, type Locator, type Page } from "@playwright/test";
import { openThemed } from "./design";

test.use({ trace: "off", screenshot: "off", video: "off" });

/* Layout of the Files page (design reference: docs/design/manager-mockup.html, #dateien), measured in a real browser. */

const box = async (target: Locator) => {
  const found = await target.boundingBox();
  if (!found) throw new Error("element has no box");
  return found;
};
const bottom = async (target: Locator) => {
  const found = await box(target);
  return found.y + found.height;
};
const list = (page: Page) => page.locator(".files-list > .lib-scroll");
const bar = (page: Page) => page.getByRole("region", { name: "Selection", exact: true });
const detail = (page: Page) => page.getByRole("complementary", { name: "Details" });
const appearance = (page: Page) => page.getByRole("group", { name: "Appearance" });
const row = (page: Page, name: string) => page.getByRole("link", { name }).locator("xpath=ancestor::tr");

test.describe("files layout", () => {
  for (const theme of ["light", "dark"] as const) {
    test(`list, selection bar and inspector end level with the appearance buttons, with and without a selection (${theme})`, async ({ page }) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await openThemed(page, "/files", theme, "Files");
      await expect(list(page)).toBeVisible();
      const anchor = await bottom(appearance(page));

      // Nothing selected: the list alone fills the work area.
      expect(Math.abs((await bottom(list(page))) - anchor)).toBeLessThan(1);

      // One file: list, selection bar and inspector.
      await row(page, "Failed Movie").click();
      await expect(detail(page)).toBeVisible();
      await expect(bar(page)).toBeVisible();
      await expect.poll(async () => Math.abs((await bottom(bar(page))) - anchor)).toBeLessThan(1);
      await expect.poll(async () => Math.abs((await bottom(detail(page))) - anchor)).toBeLessThan(1);
      // The list stops above the bar: nothing scrolls behind it.
      expect(await bottom(list(page))).toBeLessThanOrEqual((await box(bar(page))).y);

      // Several files: the same.
      await row(page, "Old Codec Movie").click({ modifiers: ["Shift"] });
      await expect(bar(page)).toContainText("selected");
      await expect.poll(async () => Math.abs((await bottom(bar(page))) - anchor)).toBeLessThan(1);
      await expect.poll(async () => Math.abs((await bottom(detail(page))) - anchor)).toBeLessThan(1);
    });
  }

  test("there are no lines between the rows, and the columns are drawn with the dashed guide", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openThemed(page, "/files", "light", "Files");
    const cells = page.getByRole("table", { name: "Media files" }).locator("tbody td");
    await expect(cells.first()).toBeVisible();
    const lines = await cells.evaluateAll((all) => all.map((cell) => {
      const style = getComputedStyle(cell);
      return Number.parseFloat(style.borderBottomWidth) + Number.parseFloat(style.borderTopWidth);
    }));
    expect(lines.length).toBeGreaterThan(3);
    expect(lines.every((width) => width === 0)).toBe(true);
    const guided = await page.getByRole("table", { name: "Media files" }).locator("tbody tr").first().locator("td").evaluateAll((all) => all.map((cell) => getComputedStyle(cell).backgroundImage !== "none"));
    expect(guided.filter(Boolean).length).toBeGreaterThanOrEqual(4);
  });

  test("the list drops Library, Encoding need and Change below 700 px of list width", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openThemed(page, "/files", "light", "Files");
    const head = page.getByRole("table", { name: "Media files" }).getByRole("columnheader");
    await expect(head).toHaveText(["File", "Library", "Status", "Encoding need", "Change", "Size"]);
    expect((await box(list(page))).width).toBeGreaterThan(700);
    for (const name of ["Library", "Encoding need", "Change"]) await expect(head.filter({ hasText: name })).toBeVisible();

    // A window where the inspector leaves the list under 700 px.
    await page.setViewportSize({ width: 1300, height: 900 });
    await row(page, "Failed Movie").click();
    await expect(detail(page)).toBeVisible();
    expect((await box(list(page))).width).toBeLessThan(700);
    await expect(head.filter({ hasText: "Status" })).toBeVisible();
    await expect(head.filter({ hasText: "Size" })).toBeVisible();
    for (const name of ["Library", "Encoding need", "Change"]) await expect(head.filter({ hasText: name })).toBeHidden();
  });

  for (const width of [1440, 1100, 560]) {
    test(`the selection bar shows its buttons whole at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await openThemed(page, "/files", "light", "Files");
      await page.setViewportSize({ width, height: 900 });
      await row(page, "Failed Movie").click();
      await row(page, "Old Codec Movie").click({ modifiers: ["Shift"] });
      await expect(bar(page)).toContainText("3 selected");
      const frame = await box(bar(page));
      const buttons = bar(page).getByRole("button");
      expect(await buttons.count()).toBe(4); // Clear selection, Verify, Re-inspect, Preview and prepare
      for (let index = 0; index < 4; index++) {
        const button = buttons.nth(index);
        const found = await box(button);
        expect(found.x, `button ${index} left`).toBeGreaterThanOrEqual(frame.x);
        expect(found.x + found.width, `button ${index} right`).toBeLessThanOrEqual(frame.x + frame.width + 0.5);
        const cut = await button.evaluate((node) => node.scrollWidth - node.clientWidth);
        expect(cut, `button ${index} text cut`).toBeLessThanOrEqual(0);
      }
    });
  }

  test("below 1180 px the inspector sits under the list, in one column", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openThemed(page, "/files", "light", "Files");
    await page.setViewportSize({ width: 1100, height: 900 });
    await row(page, "Failed Movie").click();
    await expect(detail(page)).toBeVisible();
    const upper = await box(list(page));
    const lower = await box(detail(page));
    expect(lower.y).toBeGreaterThan(upper.y + upper.height - 1);
    expect(Math.abs(lower.x - upper.x)).toBeLessThan(2);
  });
});

/* Long Sonarr/Radarr names (seeded in scripts/run-e2e.sh) must not push Status and Size out of the list. */
test.describe("files list with long names", () => {
  const within = (inner: { x: number; width: number }, outer: { x: number; width: number }) =>
    inner.x >= outer.x - 0.5 && inner.x + inner.width <= outer.x + outer.width + 0.5;

  for (const theme of ["light", "dark"] as const) {
    test(`Status and Size of every row stay inside the list with the inspector open at 1440 px (${theme})`, async ({ page }) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await openThemed(page, "/files", theme, "Files");
      await row(page, "Failed Movie").click();
      await expect(detail(page)).toBeVisible();
      const rows = page.getByRole("table", { name: "Media files" }).locator("tbody tr");
      expect(await rows.count()).toBeGreaterThanOrEqual(10);
      const frame = await box(list(page));
      expect(await list(page).evaluate((node) => node.scrollWidth - node.clientWidth), "the list scrolls sideways").toBeLessThanOrEqual(1);
      for (let index = 0; index < (await rows.count()); index++) {
        const current = rows.nth(index);
        await current.scrollIntoViewIfNeeded();
        for (const [cell, name] of [[".col-status", "Status"], [".num", "Size"]] as const) {
          const found = await box(current.locator(cell));
          expect(within(found, frame), `${name} of row ${index} lies outside the list`).toBe(true);
        }
      }
    });

    test(`the names wrap or end in an ellipsis and Status and Size stay readable at 390 px (${theme})`, async ({ page }) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await openThemed(page, "/files", theme, "Files");
      await page.setViewportSize({ width: 390, height: 844 });
      const rows = page.getByRole("table", { name: "Media files" }).locator("tbody tr");
      await expect(rows.first()).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth), "the page scrolls sideways").toBeLessThanOrEqual(0);
      const frame = await box(list(page));
      expect(await list(page).evaluate((node) => node.scrollWidth - node.clientWidth), "the list scrolls sideways").toBeLessThanOrEqual(1);
      for (let index = 0; index < (await rows.count()); index++) {
        const current = rows.nth(index);
        await current.scrollIntoViewIfNeeded();
        for (const cell of [".col-status", ".num"]) expect(within(await box(current.locator(cell)), frame), `row ${index} ${cell}`).toBe(true);
        // A name that does not fit is wrapped (at a break opportunity or, for a word that is too long, anywhere) or ends in an ellipsis: nothing is clipped.
        const clipped = await current.locator(".file-title, .file-folder").evaluateAll((all) => all.filter((node) => node.scrollWidth > node.clientWidth + 1 && getComputedStyle(node).textOverflow !== "ellipsis").length);
        expect(clipped, `row ${index} clips a name`).toBe(0);
      }
    });
  }
});

test.describe("files actions", () => {
  test("the inspector has no action button but Copy path; Verify, Re-inspect and Prepare are in the selection bar, for one file too", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openThemed(page, "/files", "light", "Files");
    await row(page, "Failed Movie").click();
    await expect(page.getByRole("heading", { name: "Failed Movie" })).toBeVisible();
    await expect(detail(page).getByRole("button")).toHaveText(["Copy path"]);
    await expect(bar(page)).toContainText("1 selected");
    for (const name of [/^Verify selection/, /^Re-inspect selection/, /^Preview and prepare/]) await expect(bar(page).getByRole("button", { name })).toBeVisible();
  });
});

test.describe("columns view", () => {
  const openColumns = async (page: Page, theme: "light" | "dark") => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openThemed(page, "/files", theme, "Files");
    await page.getByRole("button", { name: "Columns" }).click();
    const columns = page.getByRole("group", { name: /Files by folder/ });
    await columns.getByRole("listbox", { name: "Libraries" }).getByRole("option").first().click();
    await expect(columns.locator('.col[data-col="1"] [role=option]').first()).toBeVisible();
    return columns;
  };

  for (const theme of ["light", "dark"] as const) {
    test(`the dashed side line of a column is never covered, not even by a hovered or selected row (${theme})`, async ({ page }) => {
      const columns = await openColumns(page, theme);
      const second = columns.locator('.col[data-col="1"]');
      const option = second.getByRole("option").first();
      await option.click(); // selected row, and the pointer rests on it
      await option.hover();
      const result = await second.evaluate((col) => {
        const frame = col.getBoundingClientRect();
        const style = getComputedStyle(col);
        // Probe the pixel column of the line next to every row: the topmost element there must be the column itself.
        const covered = Array.from(col.querySelectorAll(".col-row")).filter((row) => {
          const box = row.getBoundingClientRect();
          const hit = document.elementFromPoint(frame.left + 0.01, box.top + box.height / 2);
          return hit !== col;
        }).length;
        return { covered, line: style.backgroundImage, border: style.borderLeftWidth + "/" + style.borderRightWidth };
      });
      expect(result.line).toContain("linear-gradient");
      expect(result.covered, JSON.stringify(result)).toBe(0);
      // The line is the guide, not a solid border.
      expect(result.border).toBe("0px/0px");
    });
  }

  test("the first column has no side line, and no column has a solid one", async ({ page }) => {
    const columns = await openColumns(page, "light");
    const lines = await columns.locator(".col").evaluateAll((all) => all.map((col) => {
      const style = getComputedStyle(col);
      return { guide: style.backgroundImage !== "none", right: Number.parseFloat(style.borderRightWidth) };
    }));
    expect(lines.length).toBeGreaterThanOrEqual(2);
    expect(lines[0]?.guide).toBe(false);
    expect(lines.slice(1).every((line) => line.guide)).toBe(true);
    expect(lines.every((line) => line.right === 0)).toBe(true);
  });
});

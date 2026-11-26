import { expect, test, type Page } from "@playwright/test";
import { openThemed } from "./design";

test.use({ trace: "off", screenshot: "off", video: "off" });

/*
 * Large lists. The default fixture is small; this journey runs only against a server seeded with
 * `ATVPREP_E2E_BULK=2000 ./scripts/run-e2e.sh e2e/files-large.spec.ts`.
 */
test.skip(!process.env.ATVPREP_E2E_BULK, "needs ATVPREP_E2E_BULK");

test("with 2000 loaded rows the list builds only a window of them and keeps the keys", async ({ page }) => {
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1440, height: 900 });
  await openThemed(page, "/files", "light", "Files");
  const table = page.getByRole("table", { name: "Media files" });
  const more = page.getByRole("button", { name: "Load more files" });
  const rows = page.locator("tr.row");
  const total = Number(process.env.ATVPREP_E2E_BULK) + 10;
  await expect(table).toHaveAttribute("aria-rowcount", "51");
  while (await more.isVisible()) {
    await more.click();
    await expect(table).not.toHaveAttribute("aria-rowcount", "51");
    await page.waitForTimeout(20);
  }
  await expect(table).toHaveAttribute("aria-rowcount", String(total + 1));
  expect(await rows.count()).toBeLessThanOrEqual(120);

  // Scrolling to the end builds the last rows and drops the first.
  const scroller = page.locator(".files-list > .lib-scroll");
  await scroller.evaluate((element) => { element.scrollTop = element.scrollHeight; });
  await expect(rows.last()).toHaveAttribute("aria-rowindex", String(total + 1));
  expect(await rows.count()).toBeLessThanOrEqual(120);

  // J and K walk on from a row that is built, and status and size stay visible next to the name.
  const last = rows.last();
  await last.click();
  await page.keyboard.press("k");
  await expect(page.locator("tr.row[aria-selected=true]")).toHaveAttribute("aria-rowindex", String(total));
  const box = await page.locator("tr.row[aria-selected=true] td").last().boundingBox();
  const area = await scroller.boundingBox();
  expect(box && area && box.x + box.width <= area.x + area.width + 1).toBe(true);
});

/** The columns view down to the folder with the bulk files; returns its last column. */
async function openBulkColumn(page: Page) {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openThemed(page, "/files", "light", "Files");
  await page.getByRole("button", { name: "Columns" }).click();
  const columns = page.getByRole("group", { name: /Files by folder/ });
  await columns.getByRole("listbox", { name: "Libraries" }).getByRole("option").first().click();
  for (const [level, name] of [[1, /^Bulk/], [2, /^Serie/], [3, /^Season 01/]] as const) {
    await columns.locator(`.col[data-col="${level}"]`).getByRole("option", { name }).first().click();
  }
  const column = columns.locator('.col[data-col="4"]');
  await expect(column.locator(".col-row").first()).toBeVisible();
  return column;
}

test("a scrolled column keeps its window and does not swing back to the top", async ({ page }) => {
  test.setTimeout(120_000);
  const column = await openBulkColumn(page);
  const rows = column.locator(".col-row");
  // The server lists at most a response worth of files per folder (about a hundred here), more than the window builds.
  const total = await column.evaluate((element) => element.scrollHeight);
  const rowHeight = (await rows.first().boundingBox())!.height;
  expect(total / rowHeight).toBeGreaterThan(80);

  // Scroll down in steps, as a wheel does; at every stop the window stays where the scrolling put it.
  const stops = [40, 55, 70, 85];
  for (const row of stops) {
    const target = Math.round(row * rowHeight);
    await column.evaluate((element, top) => { element.scrollTop = top; }, target);
    await page.waitForTimeout(150);
    // A little wheel turn afterwards makes the window follow once more.
    await column.hover();
    await page.mouse.wheel(0, 40);
    await page.waitForTimeout(150);
    const firstKeys = new Set<string | null>();
    const tops = new Set<number>();
    for (let i = 0; i < 8; i++) {
      firstKeys.add(await rows.first().getAttribute("data-key"));
      tops.add(await column.evaluate((element) => Math.round(element.scrollTop)));
      await page.waitForTimeout(60);
    }
    expect(firstKeys.size, `window at row ${row}`).toBe(1);
    expect([...tops].every((top) => Math.abs(top - target - 40) < rowHeight * 2), `scroll position at row ${row}: ${[...tops]}`).toBe(true);
    // Rows are built where the column is scrolled to.
    const area = await column.evaluate((element) => { const box = element.getBoundingClientRect(); return { top: box.top, bottom: box.bottom }; });
    const onScreen = await rows.evaluateAll((list, box) => list.filter((r) => { const b = r.getBoundingClientRect(); return b.bottom > box.top && b.top < box.bottom; }).length, area);
    expect(onScreen, `rows on screen at row ${row}`).toBeGreaterThan(5);
  }
});

test("End and Home in a column larger than its window scroll the selected row into view", async ({ page }) => {
  test.setTimeout(120_000);
  const column = await openBulkColumn(page);
  await column.getByRole("option").first().click();
  await page.keyboard.press("End");
  const selected = column.locator(".col-row.selected");
  await expect(selected).toHaveCount(1);
  await expect(selected).toBeInViewport({ ratio: 0.9 });
  await page.keyboard.press("Home");
  await expect(column.locator(".col-row.selected")).toBeInViewport({ ratio: 0.9 });
  await expect.poll(() => column.evaluate((element) => element.scrollTop)).toBeLessThan(40);
});

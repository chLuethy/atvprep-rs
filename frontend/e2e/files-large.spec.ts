import { expect, test } from "@playwright/test";
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

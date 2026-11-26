import { expect, test, type Page } from "@playwright/test";
import { openThemed, type Theme } from "./design";

test.use({ trace: "off", screenshot: "off", video: "off" });

/* The log of an item is set out in the page and stays the plain text for the download. */

async function openLog(page: Page, theme: Theme, width: number) {
  // Unlock wide (the Lock button hides in the narrow menu), then measure at the width asked for.
  await page.setViewportSize({ width: 1440, height: 900 });
  await openThemed(page, "/jobs", theme, "Activity");
  await page.setViewportSize({ width, height: 900 });
  await page.getByRole("region", { name: "History" }).getByRole("link", { name: /Old Codec Movie/ }).first().click();
  await page.getByRole("button", { name: "Log", exact: true }).click();
  const log = page.getByRole("region", { name: "Formatted log" });
  await expect(log).toBeVisible();
  return log;
}

test.describe("item log", () => {
  for (const theme of ["light", "dark"] as const) {
    for (const width of [1440, 390]) {
      test(`is set out, and nothing runs sideways (${theme}, ${width} px)`, async ({ page }) => {
        const log = await openLog(page, theme, width);

        await expect(log.getByText("Phase verify")).toBeVisible();
        await expect(log.getByText("Check passed")).toBeVisible();
        await expect(log.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "99");
        await expect(log).not.toContainText("ATVPREP-");
        await expect(log).not.toContainText('"phase"');

        const overflow = await page.evaluate(() => {
          const root = document.scrollingElement as HTMLElement;
          const region = document.querySelector<HTMLElement>('[aria-label="Formatted log"]')!;
          return { page: root.scrollWidth - root.clientWidth, log: region.scrollWidth - region.clientWidth };
        });
        expect(overflow.page).toBeLessThanOrEqual(0);
        expect(overflow.log).toBeLessThanOrEqual(0);
      });
    }
  }

  test("shows the plain text on request and downloads it unchanged", async ({ page }) => {
    await openLog(page, "light", 1440);
    await page.getByRole("button", { name: "Raw text" }).click();
    const raw = page.getByLabel("Plain text log");
    await expect(raw).toContainText("ATVPREP-PROGRESS 87");
    await expect(raw).toContainText('{"bytes":3645981583,"phase":"verify"');

    const download = page.waitForEvent("download");
    await page.getByRole("button", { name: "Download log" }).click();
    const file = await download;
    const chunks: Buffer[] = [];
    for await (const chunk of (await file.createReadStream())!) chunks.push(Buffer.from(chunk));
    const text = Buffer.concat(chunks).toString("utf8");
    expect(text).toContain("ATVPREP-PROGRESS 87");
    expect(text).toContain("[00:14:20] Phase verify: started 00:14:19, took 1:11");
  });
});

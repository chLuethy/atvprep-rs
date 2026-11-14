import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("view memory journey", () => {
  test("filters survive the page change, a reload and Reset filters forgets them", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();
    const bar = page.getByRole("search", { name: "Filter files" });

    await bar.getByRole("button", { name: "Status", exact: true }).click();
    await page.getByRole("dialog", { name: "Status" }).getByRole("checkbox", { name: /^Warning/ }).check();
    await page.keyboard.press("Escape");
    await page.getByLabel("Search files").fill("Failed");
    await expect(page).toHaveURL(/\/files\?status=warning&q=Failed$/);

    const nav = page.getByRole("navigation", { name: "Main" });
    await nav.getByRole("link", { name: /Activity/ }).click();
    await expect(page).toHaveURL(/\/jobs$/);
    await page.getByRole("search", { name: "Filter jobs" }).getByRole("button", { name: "Status", exact: true }).click();
    await page.getByRole("dialog", { name: "Status" }).getByRole("checkbox", { name: /^Failed/ }).check();
    await page.keyboard.press("Escape");
    await expect(page).toHaveURL(/\/jobs\?status=failed$/);

    await nav.getByRole("link", { name: /Files/ }).click();
    await expect(page).toHaveURL(/\/files\?status=warning&q=Failed$/);
    await expect(page.getByLabel("Search files")).toHaveValue("Failed");
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Sample Movie" })).toHaveCount(0);

    await nav.getByRole("link", { name: /Activity/ }).click();
    await expect(page).toHaveURL(/\/jobs\?status=failed$/);

    // A reload of the bare address brings the state back too.
    await page.goto("/files");
    await expect(page).toHaveURL(/\/files\?status=warning&q=Failed$/);

    await page.getByRole("button", { name: "Reset filters" }).first().click();
    await expect(page).toHaveURL(/\/files$/);
    await nav.getByRole("link", { name: /Activity/ }).click();
    await nav.getByRole("link", { name: /Files/ }).click();
    await expect(page).toHaveURL(/\/files$/);
    await expect(page.getByRole("link", { name: "Sample Movie" })).toBeVisible();
  });
});

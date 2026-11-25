import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("command palette finds files, jobs and schedules", () => {
  test("empty input lists the commands as before", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await page.keyboard.press("ControlOrMeta+k");
    await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("option")).toHaveCount(22);
  });

  test("\"maisel\" finds the Maisel files, highlights the hit and Enter opens the chosen one", async ({ page }) => {
    await openAndUnlock(page, "/jobs");
    await page.keyboard.press("ControlOrMeta+k");
    const search = page.getByRole("combobox", { name: "Search commands" });
    await search.fill("maisel");
    const hits = page.getByRole("dialog", { name: "Command palette" }).getByRole("option");
    // The two files first; a job that holds one of them may follow.
    await expect(hits.nth(1)).toBeVisible();
    await expect(hits.first()).toContainText("S01E01");
    await expect(hits.nth(1)).toContainText("S01E02");
    await expect(hits.first().locator("mark").first()).toHaveText(/maisel/i);
    await search.press("ArrowDown");
    await search.press("Enter");
    await expect(page).toHaveURL(/\/files\/.+/);
    await expect(page.getByRole("heading", { name: /S01E02/ })).toBeVisible();
  });

  test("\"S01E02\" finds exactly that episode", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await page.keyboard.press("ControlOrMeta+k");
    const search = page.getByRole("combobox", { name: "Search commands" });
    await search.fill("S01E02");
    const hits = page.getByRole("dialog", { name: "Command palette" }).getByRole("option");
    await expect(hits.first()).toContainText("S01E02");
    await expect(hits.first()).not.toContainText("S01E01");
    await search.press("Enter");
    await expect(page).toHaveURL(/\/files\/.+/);
    await expect(page.getByRole("heading", { name: /S01E02/ })).toBeVisible();
  });

  test("a schedule is reachable through the palette", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await page.keyboard.press("ControlOrMeta+k");
    const search = page.getByRole("combobox", { name: "Search commands" });
    await search.fill("E2E Weekly");
    await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("option")).toHaveCount(1);
    await search.press("Enter");
    await expect(page).toHaveURL(/\/schedules/);
  });

  test("a job is reachable through the palette", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await page.keyboard.press("ControlOrMeta+k");
    const search = page.getByRole("combobox", { name: "Search commands" });
    await search.fill("Failed Movie");
    const job = page.getByRole("dialog", { name: "Command palette" }).getByRole("option").filter({ hasText: "Job" });
    await expect(job.first()).toBeVisible();
    await job.first().click();
    await expect(page).toHaveURL(/\/jobs\/.+/);
  });
});

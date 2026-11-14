import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("activity period and library", () => {
  test("narrows by period and library under More filters and keeps both in the address", async ({ page }) => {
    await openAndUnlock(page, "/jobs");
    const bar = page.getByRole("search", { name: "Filter jobs" });
    await expect(bar.getByText(/^\d+ of \d+ jobs$/)).toBeVisible();

    await bar.getByRole("button", { name: "More filters" }).click();
    await bar.getByRole("button", { name: "Period", exact: true }).click();
    const period = page.getByRole("dialog", { name: "Period" });
    await period.getByRole("radio", { name: "Today" }).check();
    await expect(page).toHaveURL(/\/jobs\?range=today$/);
    // The fixture went in today.
    await expect(page.getByRole("link", { name: /Failed Movie/ }).first()).toBeVisible();
    await expect(bar.getByRole("button", { name: "More filters" })).toHaveAttribute("aria-disabled", "true");

    await period.getByRole("radio", { name: "Custom period" }).check();
    await period.getByLabel("From", { exact: true }).fill("2020-01-01");
    await period.getByLabel("To", { exact: true }).fill("2020-01-02");
    await expect(page).toHaveURL(/\/jobs\?from=2020-01-01&to=2020-01-02$/);
    await expect(page.getByText("No jobs match the current filters")).toBeVisible();
    await page.keyboard.press("Escape");

    await page.reload();
    await expect(bar.getByRole("button", { name: /^Period: .+2020/ })).toBeVisible();
    await bar.getByRole("button", { name: "Clear Period" }).click();
    await expect(page).toHaveURL(/\/jobs$/);

    await bar.getByRole("button", { name: "More filters" }).click();
    await bar.getByRole("button", { name: "Library", exact: true }).click();
    await page.getByRole("dialog", { name: "Library" }).getByRole("checkbox", { name: /^media, \d+ jobs?$/ }).check();
    await expect(page).toHaveURL(/\/jobs\?root_id=[^&]+$/);
    await expect(page.getByRole("link", { name: /Failed Movie/ }).first()).toBeVisible();
  });

  test("on a narrow screen the facets scroll in one row, More filters included, and a popover takes the whole screen", async ({ page }) => {
    await openAndUnlock(page, "/jobs?range=today");
    await page.setViewportSize({ width: 390, height: 844 });
    const bar = page.getByRole("search", { name: "Filter jobs" });
    await expect(bar.getByRole("button", { name: "All filters (1 active)" })).toBeVisible();
    // The filter symbol first, then the active period from More filters, then the rest.
    const left = async (name: string | RegExp) => (await bar.getByRole("button", { name, exact: typeof name === "string" }).boundingBox())!.x;
    // The narrow layout follows the resize; wait until the period has joined the row.
    await expect.poll(async () => (await left("Status")) > (await left(/^Period: /))).toBe(true);
    expect(await left("All filters (1 active)")).toBeLessThan(await left(/^Period: /));
    // Search, sort and the facet row fit the width; only the facet row scrolls.
    expect(await bar.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    const row = bar.locator(".fb-facets").first();
    expect(await row.evaluate((element) => element.scrollWidth > element.clientWidth)).toBe(true);

    await bar.getByRole("button", { name: "Status", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Status" });
    const box = (await dialog.boundingBox())!;
    expect(box.width).toBeGreaterThanOrEqual(389);
    expect(box.height).toBeGreaterThanOrEqual(843);
    await dialog.getByRole("checkbox", { name: /^Failed, \d+ jobs?/ }).check();
    await expect(page).toHaveURL(/\/jobs\?status=failed&range=today$/);
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(bar.getByRole("button", { name: "All filters (2 active)" })).toBeVisible();
  });
});

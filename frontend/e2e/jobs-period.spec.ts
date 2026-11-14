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
    await expect(bar.getByRole("button", { name: "More filters" })).toBeDisabled();

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
    await page.getByRole("dialog", { name: "Library" }).getByRole("checkbox", { name: /^media \d+$/ }).check();
    await expect(page).toHaveURL(/\/jobs\?root_id=[^&]+$/);
    await expect(page.getByRole("link", { name: /Failed Movie/ }).first()).toBeVisible();
  });
});

import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("settings journey", () => {
  test("edits an audited runtime setting and preserves the revision contract", async ({ page }) => {

    await openAndUnlock(page, "/settings");
    await expect(page.getByRole("heading", { name: "Runtime settings" })).toBeVisible();
    const interval = page.getByLabel("Scan interval (seconds)", { exact: true });
    await expect(interval).toBeVisible();
    await interval.fill("10");
    await page.getByRole("button", { name: "Save settings" }).click();
    await expect(page.getByText("Settings saved.", { exact: true })).toBeVisible();

    await page.getByRole("button", { name: "Show history of Scan interval (seconds)" }).click();
    await expect(page.getByRole("region", { name: "Scan interval (seconds) history" })).toBeVisible();
  });
});

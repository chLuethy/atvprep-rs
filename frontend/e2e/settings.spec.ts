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

  test("lays the groups out in two columns over the full width, one column below 1100 px", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openAndUnlock(page, "/settings");
    const scan = page.getByRole("region", { name: "Library scan" });
    const target = page.getByRole("region", { name: "Target profile" });
    const automation = page.getByRole("region", { name: "Automation" });
    const retention = page.getByRole("region", { name: "Retention" });
    await expect(scan).toBeVisible();
    const box = async (locator: typeof scan) => (await locator.boundingBox())!;
    const [scanBox, retentionBox, targetBox, automationBox] = [await box(scan), await box(retention), await box(target), await box(automation)];
    // Scan and retention on the left, automation and target profile on the right, side by side.
    expect(targetBox.x).toBeGreaterThan(scanBox.x + scanBox.width - 1);
    expect(automationBox.x).toBeCloseTo(targetBox.x, 0);
    expect(retentionBox.x).toBeCloseTo(scanBox.x, 0);
    // The groups use the width: no large blank area to the right of the form.
    const content = (await page.locator(".content").boundingBox())!;
    expect(targetBox.x + targetBox.width).toBeGreaterThan(content.x + content.width - 60);
    // The save bar sits below both columns over the full width (it is sticky: scroll to the end first).
    await page.locator(".settings").evaluate((el) => { el.scrollTop = el.scrollHeight; });
    const [scanBox2, retentionBox2, targetBox2] = [await box(scan), await box(retention), await box(target)];
    const bar = (await page.locator(".savebar").boundingBox())!;
    expect(bar.y).toBeGreaterThan(Math.max(scanBox2.y + scanBox2.height, retentionBox2.y + retentionBox2.height, targetBox2.y + targetBox2.height) - 1);
    expect(bar.width).toBeGreaterThan(content.width * 0.9);

    await page.setViewportSize({ width: 1000, height: 900 });
    await page.locator(".settings").evaluate((el) => { el.scrollTop = 0; });
    const narrowScan = await box(scan);
    const narrowTarget = await box(target);
    expect(narrowTarget.x).toBeCloseTo(narrowScan.x, 0);
    expect(narrowTarget.y).toBeGreaterThan(narrowScan.y);
  });
});

import { expect, test } from "@playwright/test";
import { unlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("authentication journey", () => {
  test("rejects a bad token, unlocks, and locks without persisting the secret", async ({ page }) => {
    await page.goto("/");
    await page.getByLabel("Master token").fill("definitely-not-the-master-token");
    await page.getByRole("button", { name: "Unlock" }).click();
    await expect(page.getByRole("alert")).toHaveText("Unable to unlock manager session.");
    await expect(page.getByLabel("Master token")).toHaveValue("");

    await unlock(page);
    await expect(page.getByRole("navigation", { name: "Main" })).toBeVisible();
    await page.getByRole("button", { name: "Lock", exact: true }).click();
    await expect(page.getByRole("heading", { name: "Unlock manager" })).toBeVisible();
    await expect(page.getByLabel("Master token")).toHaveValue("");
    await expect.poll(() => page.evaluate(async () => (await fetch("/api/v1/dashboard")).status)).toBe(401);
  });
});

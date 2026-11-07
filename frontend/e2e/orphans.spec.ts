import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("leftover partial rewrites journey", () => {
  test("the overview reports them and removes them only after a confirmation", async ({ page }) => {
    await openAndUnlock(page, "/dashboard");
    const section = page.getByRole("region", { name: "Leftover files" });
    await expect(section.getByText("1 leftover file in media (15 B)")).toBeVisible();
    await expect(section.getByText(/\.tmp_fixmeta_enc\.Sample Movie\.4711-0a1b2c3d\.mp4/)).toBeVisible();

    await section.getByRole("button", { name: "Remove leftovers" }).click();
    const confirm = page.getByRole("region", { name: "Confirm removing leftovers" });
    await expect(confirm.getByText("Delete 1 file, 15 B in total?")).toBeVisible();
    await confirm.getByRole("button", { name: "Delete leftovers" }).click();

    await expect(page.getByText("Removed 1 leftover file (15 B).")).toBeVisible();
    await expect(page.getByRole("region", { name: "Leftover files" })).toHaveCount(0);
    await expect(page.getByRole("region", { name: "Recent activity" }).getByText("Leftover files removed")).toBeVisible();

    await page.reload();
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    await expect(page.getByRole("region", { name: "Leftover files" })).toHaveCount(0);
  });
});

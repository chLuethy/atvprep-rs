import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("activity journey", () => {
  test("names each job by its file, explains a failure, and cancels and retries inline", async ({ page }) => {
    await openAndUnlock(page, "/jobs");
    await expect(page.getByRole("heading", { name: "Activity" })).toBeVisible();

    const active = page.getByRole("region", { name: "Active" });
    // Earlier specs may have queued more jobs for the same file; any of them will do.
    await expect(active.getByRole("link", { name: /Sample Movie/ }).first()).toBeVisible();
    const history = page.getByRole("region", { name: "History" });
    await expect(history.getByRole("link", { name: /Failed Movie/ }).first()).toBeVisible();

    await active.getByRole("link", { name: /Sample Movie/ }).first().click();
    await expect(page.getByRole("heading", { name: "Sample Movie" })).toBeVisible();
    await page.getByRole("button", { name: "Cancel job" }).click();
    await expect(page.getByRole("region", { name: "Confirm cancel" })).toBeVisible();
    await page.getByRole("button", { name: "Confirm cancel" }).click();
    await expect(page.getByText("Job cancellation requested")).toBeVisible();

    await page.getByRole("region", { name: "History" }).getByRole("link", { name: /Failed Movie/ }).first().click();
    await expect(page.getByRole("heading", { name: "Failed Movie" })).toBeVisible();
    const why = page.getByRole("region", { name: "Why it failed" });
    await expect(why).toContainText("synthetic_failure");
    await expect(why).toContainText("synthetic e2e failure");

    await page.getByRole("button", { name: "Retry failed items" }).click();
    await expect(page.getByRole("region", { name: "Confirm retry" })).toBeVisible();
    await page.getByRole("button", { name: "Confirm retry" }).click();
    await expect(page.getByRole("status")).toContainText("Retry queued");
  });
});

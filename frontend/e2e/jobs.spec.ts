import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("activity journey", () => {
  test("the filter bar narrows by status, keeps it in the address, reports hidden active jobs and Back restores it", async ({ page }) => {
    await openAndUnlock(page, "/jobs");
    const bar = page.getByRole("search", { name: "Filter jobs" });
    await expect(bar.getByText(/^\d+ of \d+ jobs$/)).toBeVisible();
    await expect(page.getByRole("region", { name: "Active" })).toBeVisible();

    await bar.getByRole("button", { name: "Status", exact: true }).click();
    await page.getByRole("dialog", { name: "Status" }).getByRole("checkbox", { name: /^Failed \d+$/ }).check();
    await page.keyboard.press("Escape");
    await expect(page).toHaveURL(/\/jobs\?status=failed$/);
    await expect(page.getByRole("region", { name: "History" }).getByRole("link", { name: /Failed Movie/ }).first()).toBeVisible();
    await expect(page.getByRole("region", { name: "Active" })).toHaveCount(0);
    const hidden = page.getByRole("status", { name: "Hidden active jobs" });
    await expect(hidden).toContainText(/\d+ active jobs? hidden/);

    await hidden.getByRole("button", { name: "Show them" }).click();
    await expect(page).toHaveURL(/status=queued,running,cancel_requested,failed$/);
    await expect(page.getByRole("region", { name: "Active" })).toBeVisible();
    await expect(hidden).toHaveCount(0);

    await page.goBack();
    await expect(page).toHaveURL(/\/jobs\?status=failed$/);
    await expect(bar.getByRole("button", { name: "Status: Failed" })).toBeVisible();

    await page.getByRole("combobox", { name: "Order" }).selectOption("oldest");
    await expect(page).toHaveURL(/status=failed&sort=oldest$/);
    await page.getByLabel("Search jobs").fill("Failed Movie");
    await expect(page).toHaveURL(/status=failed&q=Failed%20Movie&sort=oldest$/);
    await page.reload();
    await expect(page.getByLabel("Search jobs")).toHaveValue("Failed Movie");
    await expect(page.getByRole("combobox", { name: "Order" })).toHaveValue("oldest");

    await bar.getByRole("button", { name: "Reset filters" }).click();
    await expect(page).toHaveURL(/\/jobs\?sort=oldest$/);
  });

  test("the Auslöser tells jobs from a schedule apart from those by hand and keeps the choice in the address", async ({ page }) => {
    await openAndUnlock(page, "/jobs");
    const bar = page.getByRole("search", { name: "Filter jobs" });
    await bar.getByRole("button", { name: "Trigger", exact: true }).click();
    const popover = page.getByRole("dialog", { name: "Trigger" });
    const schedules = popover.getByRole("group", { name: "Schedules" });
    await expect(schedules.getByRole("checkbox", { name: "Schedules 1" })).toBeVisible();
    // The schedule's job is stored as manual, yet it does not count as by hand.
    await expect(popover.getByRole("checkbox", { name: /^By hand \d+$/ })).toBeVisible();

    await schedules.getByRole("searchbox", { name: "Search Schedules" }).fill("weekly");
    await schedules.getByRole("checkbox", { name: "E2E Weekly 1" }).check();
    await expect(page).toHaveURL(/\/jobs\?schedule=sc_[\w-]+$/);
    const history = page.getByRole("region", { name: "History" });
    await expect(history.getByRole("link", { name: /Old Codec Movie/ })).toBeVisible();
    await expect(history.getByRole("link", { name: /Failed Movie/ })).toHaveCount(0);
    await expect(bar.getByText(/^1 of \d+ jobs$/)).toBeVisible();

    await popover.getByRole("checkbox", { name: /^By hand \d+$/ }).check();
    await expect(page).toHaveURL(/\/jobs\?trigger=manual&schedule=sc_[\w-]+$/);
    await page.keyboard.press("Escape");
    await page.reload();
    await expect(bar.getByRole("button", { name: "Trigger: By hand, E2E Weekly" })).toBeVisible();
    await expect(bar.getByText(/^\d+ of \d+ jobs$/)).toBeVisible();

    await page.goBack();
    await expect(page).toHaveURL(/\/jobs\?schedule=sc_[\w-]+$/);
    await expect(bar.getByRole("button", { name: "Trigger: E2E Weekly" })).toBeVisible();
  });

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
    // The fixture stores this job under its former name `fix`; the history shows it as Prepare.
    await expect(page.locator("dt", { hasText: "Action" }).locator("xpath=following-sibling::dd[1]")).toHaveText("Prepare");
    const why = page.getByRole("region", { name: "Why it failed" });
    await expect(why).toContainText("synthetic_failure");
    await expect(why).toContainText("synthetic e2e failure");

    await page.getByRole("button", { name: "Retry failed items" }).click();
    await expect(page.getByRole("region", { name: "Confirm retry" })).toBeVisible();
    await page.getByRole("button", { name: "Confirm retry" }).click();
    await expect(page.getByRole("status")).toContainText("Retry queued");
  });
});

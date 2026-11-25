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
    await page.getByRole("dialog", { name: "Status" }).getByRole("checkbox", { name: /^Failed, \d+ jobs?, needs attention$/ }).check();
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

  test("a quick filter sets its filter, shows as in effect, is in the command palette and the overview leads there", async ({ page }) => {
    await openAndUnlock(page, "/jobs?q=Heat");
    const bar = page.getByRole("search", { name: "Filter jobs" });
    const quick = bar.getByRole("group", { name: "Quick filters" });
    const failed = quick.getByRole("button", { name: "Failed in 24 h" });
    await failed.click();
    await expect(page).toHaveURL(/\/jobs\?status=partial_success,failed&range=24h$/);
    await expect(failed).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByLabel("Search jobs")).toHaveValue("");
    await expect(page.getByRole("region", { name: "History" }).getByRole("link", { name: /Failed Movie/ }).first()).toBeVisible();
    await expect(page.getByRole("region", { name: "Active" })).toHaveCount(0);

    await page.keyboard.press("ControlOrMeta+k");
    const search = page.getByRole("combobox", { name: "Search commands" });
    await search.fill("activity: running");
    await search.press("Enter");
    await expect(page).toHaveURL(/\/jobs\?status=queued,running,cancel_requested$/);
    await expect(quick.getByRole("button", { name: "Running" })).toHaveAttribute("aria-pressed", "true");
    await expect(failed).toHaveAttribute("aria-pressed", "false");
    await expect(page.getByRole("region", { name: "Active" })).toBeVisible();

    await page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Overview" }).click();
    await page.getByRole("region", { name: "Needs you" }).getByRole("link", { name: /jobs? failed$/ }).click();
    await expect(page).toHaveURL(/\/jobs\?status=failed$/);
    await expect(bar.getByRole("button", { name: "Status: Failed" })).toBeVisible();
  });

  test("the Auslöser tells jobs from a schedule apart from those by hand and keeps the choice in the address", async ({ page }) => {
    await openAndUnlock(page, "/jobs");
    const bar = page.getByRole("search", { name: "Filter jobs" });
    await bar.getByRole("button", { name: "Trigger", exact: true }).click();
    const popover = page.getByRole("dialog", { name: "Trigger" });
    const schedules = popover.getByRole("group", { name: "Schedules" });
    await expect(schedules.getByRole("checkbox", { name: "Schedules, 1 job" })).toBeVisible();
    // The schedule's job is stored as manual, yet it does not count as by hand.
    await expect(popover.getByRole("checkbox", { name: /^By hand, \d+ jobs?$/ })).toBeVisible();

    await schedules.getByRole("searchbox", { name: "Search Schedules" }).fill("weekly");
    await schedules.getByRole("checkbox", { name: "E2E Weekly, 1 job" }).check();
    await expect(page).toHaveURL(/\/jobs\?schedule_id=sc_[\w-]+$/);
    const history = page.getByRole("region", { name: "History" });
    await expect(history.getByRole("link", { name: /Old Codec Movie/ })).toBeVisible();
    await expect(history.getByRole("link", { name: /Failed Movie/ })).toHaveCount(0);
    await expect(bar.getByText(/^1 of \d+ jobs$/)).toBeVisible();

    await popover.getByRole("checkbox", { name: /^By hand, \d+ jobs?$/ }).check();
    await expect(page).toHaveURL(/\/jobs\?trigger=manual&schedule_id=sc_[\w-]+$/);
    await page.keyboard.press("Escape");
    await page.reload();
    await expect(bar.getByRole("button", { name: "Trigger: By hand, E2E Weekly" })).toBeVisible();
    await expect(bar.getByText(/^\d+ of \d+ jobs$/)).toBeVisible();

    await page.goBack();
    await expect(page).toHaveURL(/\/jobs\?schedule_id=sc_[\w-]+$/);
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
    await expect(why).toContainText("The job failed.");
    await expect(why.getByText("synthetic_failure")).toBeHidden();
    await expect(why).toContainText("synthetic e2e failure");

    await page.getByRole("button", { name: "Retry failed items" }).click();
    await expect(page.getByRole("region", { name: "Confirm retry" })).toBeVisible();
    await page.getByRole("button", { name: "Confirm retry" }).click();
    await expect(page.getByRole("status")).toContainText("Retry queued");
  });
  test("puts the detail beside the list, and under it below 1180 px, with widths in classes", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openAndUnlock(page, "/jobs");
    await page.getByRole("region", { name: "History" }).getByRole("link", { name: /Failed Movie/ }).first().click();
    await expect(page.getByRole("heading", { name: "Failed Movie" })).toBeVisible();
    const list = page.getByRole("region", { name: "Activity" });
    const detail = page.getByRole("complementary", { name: "Job details" });
    const box = async (locator: typeof list) => (await locator.boundingBox())!;

    const wideList = await box(list);
    const wideDetail = await box(detail);
    expect(wideDetail.x).toBeGreaterThan(wideList.x + wideList.width - 1);
    expect(Math.abs(wideDetail.y - wideList.y)).toBeLessThan(40);
    // The detail takes the rest of the width; the list stays a column.
    expect(wideDetail.width).toBeGreaterThan(wideList.width);

    // Column widths are classes, so a media query can change them: nothing on the page sets a width inline.
    expect(await page.locator(".activity [style*='width'], .activity [style*='grid-template']").count()).toBe(0);

    await page.setViewportSize({ width: 1000, height: 900 });
    const narrowList = await box(list);
    const narrowDetail = await box(detail);
    expect(narrowDetail.x).toBeCloseTo(narrowList.x, 0);
    expect(narrowDetail.y).toBeGreaterThan(narrowList.y + narrowList.height - 1);
    await expect(page.getByRole("heading", { name: "Failed Movie" })).toBeVisible();
    // The page itself scrolls to the detail; no pane is cut off.
    const cut = await page.locator(".activity").evaluate((el) => el.scrollWidth - el.clientWidth);
    expect(cut).toBeLessThanOrEqual(0);
  });
});

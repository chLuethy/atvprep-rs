import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test("behält offenes Protokoll und Fokus bei Item-Nachladefehler und verzögerter Wiederholung", async ({ page }) => {
  let attempts = 0;
  let release!: () => void;
  const ready = new Promise<void>((resolve) => { release = resolve; });
  const job = {
    id: "j_paged", action: "verify", status: "succeeded", trigger: "manual", origin_job_id: null,
    requested_at: "2026-11-14T08:00:00Z", started_at: "2026-11-14T08:01:00Z", finished_at: "2026-11-14T08:02:00Z",
    total_count: 2, succeeded_count: 2, failed_count: 0, cancelled_count: 0, subject: "First Browser Item.mp4",
  };
  const item = (position: number, display_path: string) => ({
    id: `i_paged_${position}`, job_id: "j_paged", media_id: `m_paged_${position}`, action: "verify", position,
    status: "succeeded", attempt: 1, output_media_id: null, display_path, error_code: null, error_detail: null,
  });
  await page.route("**/api/v1/jobs/j_paged", (route) => route.fulfill({ json: { data: job } }));
  await page.route(/\/api\/v1\/jobs\/j_paged\/items\?/, async (route) => {
    if (!new URL(route.request().url()).searchParams.has("cursor")) {
      await route.fulfill({ json: { data: [item(1, "First Browser Item.mp4")], meta: { next_cursor: "browser-next" } } });
      return;
    }
    attempts += 1;
    if (attempts === 1) {
      await route.fulfill({ status: 503, json: { detail: "Vorübergehender Nachladefehler" } });
      return;
    }
    await ready;
    await route.fulfill({ json: { data: [item(2, "Second Browser Item.mp4")], meta: {} } });
  });
  await page.route("**/api/v1/job-items/i_paged_1/log", (route) => route.fulfill({
    json: { data: { item_id: "i_paged_1", status: "succeeded", log_tail: "Bereits geöffnetes Protokoll" } },
  }));
  try {
    await openAndUnlock(page, "/jobs/j_paged");
    const first = page.locator(".item-row").filter({ hasText: "First Browser Item" });
    const toggle = first.getByRole("button", { name: "Log", exact: true });
    await toggle.click();
    const log = page.getByLabel("Plain text log");
    await expect(log).toContainText("Bereits geöffnetes Protokoll");
    await page.getByRole("button", { name: "Load more items" }).click();
    const failure = page.getByRole("alert").filter({ hasText: "Unable to load more items" });
    await expect(failure).toBeVisible();
    await expect(first).toBeVisible();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(log).toContainText("Bereits geöffnetes Protokoll");
    const original = await toggle.elementHandle();
    await failure.getByRole("button", { name: "Retry", exact: true }).click();
    await expect.poll(() => attempts).toBe(2);
    await toggle.focus();
    await expect(toggle).toBeFocused();
    release();
    await expect(page.locator(".item-row")).toHaveCount(2);
    await expect(page.getByText("Second Browser Item", { exact: true })).toBeVisible();
    await expect(failure).toHaveCount(0);
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(toggle).toBeFocused();
    await expect(log).toContainText("Bereits geöffnetes Protokoll");
    expect(await toggle.evaluate((node, original) => node === original, original)).toBe(true);
    await original?.dispose();
  } finally { release(); }
});

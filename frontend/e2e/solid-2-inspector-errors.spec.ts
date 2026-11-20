import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test("behält Dateidetails beim Wiederholen eines Inspektionsladefehlers", async ({ page }) => {
  let attempts = 0;
  let release!: () => void;
  const ready = new Promise<void>((resolve) => { release = resolve; });
  await page.route(/\/api\/v1\/files\/[^/]+\/inspections\?/, async (route) => {
    attempts += 1;
    if (attempts === 1) {
      await route.fulfill({ status: 503, json: { detail: "Vorübergehender Inspektionsladefehler" } });
      return;
    }
    const response = await route.fetch({ headers: { ...await route.request().allHeaders(), Origin: new URL(page.url()).origin } });
    await ready;
    await route.fulfill({ response });
  });
  try {
    await openAndUnlock(page, "/files");
    await page.getByRole("link", { name: "Sample Movie", exact: true }).click();
    const heading = page.getByRole("heading", { name: "Sample Movie", exact: true });
    await expect(heading).toBeVisible();
    const failure = page.getByRole("alert").filter({ hasText: "Unable to load inspections" });
    await expect(failure).toBeVisible();
    await failure.getByRole("button", { name: "Retry", exact: true }).click();
    await expect.poll(() => attempts).toBe(2);
    release();
    await expect(failure).toHaveCount(0);
    await expect(page.getByText("No tracks yet: the file has not been inspected.", { exact: true })).toBeVisible();
    await expect(heading).toBeVisible();
  } finally { release(); }
});

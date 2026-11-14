import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("dashboard journey", () => {
  test("loads summary, activity, queue, and survives a deep-link reload", async ({ page }) => {
    await page.addInitScript(() => {
      const NativeWebSocket = window.WebSocket;
      let attempts = 0;
      (window as unknown as { __e2eWsAttempts: () => number }).__e2eWsAttempts = () => attempts;
      window.WebSocket = class extends NativeWebSocket {
        constructor(url: string | URL, protocols?: string | string[]) {
          super(url, protocols);
          attempts += 1;
          if (attempts === 1) setTimeout(() => {
            this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify({ type: "gap", after: null, reason: "e2e" }) }));
            this.close();
          }, 50);
        }
      } as typeof WebSocket;
    });
    const resyncResponse = page.waitForResponse((response) => response.request().method() === "GET" && response.url().includes("/api/v1/events"));
    await openAndUnlock(page, "/dashboard");
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    await expect(page.getByRole("region", { name: "File health" })).toBeVisible();
    await expect(page.getByRole("region", { name: "Recent activity" })).toBeVisible();
    await expect(page.getByRole("region", { name: "Running now" })).toBeVisible();
    await expect.poll(() => page.evaluate(() => (window as unknown as { __e2eWsAttempts: () => number }).__e2eWsAttempts())).toBeGreaterThan(1);
    expect((await resyncResponse).status()).toBe(200);
    await expect(page.getByText("Job queued").first()).toBeVisible();

    await page.reload();
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
  });
});

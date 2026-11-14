/**
 * Smoke test for the atvprep-manager SPA served by the Rust manager.
 *
 * This is a *smoke* test, not a full UI test: it asserts the basics that
 * prove the server is serving the built SolidJS SPA:
 *
 *   1. The root page returns 200 and the HTML title is "atvprep-manager".
 *   2. The SPA actually hydrates: the five nav links
 *      (Overview / Files / Activity / Schedules / Settings) become anchors with
 *      the expected `href` values.
 *
 * Run it, together with the other journeys, from the repository root:
 *
 *   ./scripts/run-e2e.sh
 *
 * which builds the SPA and the binary, seeds a temporary database, starts the server and runs
 * Playwright (needs `npx playwright install chromium` once).
 */

import { expect, test } from "@playwright/test";

test.use({ trace: "off", screenshot: "off", video: "off" });

async function unlock(page: import("@playwright/test").Page): Promise<void> {
  await page.getByLabel("Master token").fill(process.env.E2E_MASTER_TOKEN ?? "");
  await page.getByRole("button", { name: "Unlock" }).click();
  await expect(page.getByRole("button", { name: "Lock", exact: true })).toBeVisible();
}

test.describe("atvprep-manager SPA smoke (server-served)", () => {
  test("root page has the expected title", async ({ page }) => {
    const response = await page.goto("/");
    expect(response, "GET / should return a response").not.toBeNull();
    expect(response!.status(), "GET / should be 200").toBe(200);
    await expect(page).toHaveTitle("atvprep-manager");
  });

  test("SPA hydrates and renders the five nav links", async ({ page }) => {
    await page.goto("/");
    await unlock(page);
    // The nav is rendered by SolidJS after hydration; wait for it.
    const nav = page.getByRole("navigation", { name: "Main" });
    await expect(nav, "main nav should be present after hydration").toBeVisible();

    const links = nav.getByRole("link");
    await expect(links).toHaveCount(5);

    const hrefs = await links.evaluateAll((els) =>
      els.map((el) => (el as HTMLAnchorElement).getAttribute("href")),
    );
    expect(hrefs).toEqual(
      expect.arrayContaining(["/dashboard", "/files", "/jobs", "/schedules", "/settings"]),
    );
  });

  test("client-side route /dashboard renders the Overview section", async ({ page }) => {
    await page.goto("/dashboard");
    await unlock(page);
    await expect(page.getByText("atvprep-manager", { exact: true })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Overview" })).toHaveAttribute("aria-current", "page");
  });

  test("health endpoint is reachable through the same origin", async ({ request }) => {
    const resp = await request.get("/api/health/live");
    expect(resp.status(), "/api/health/live should be 200").toBe(200);
    expect(await resp.json()).toEqual({ status: "alive" });
  });
});

/**
 * Playwright config for the atvprep-manager SPA journeys.
 *
 * Target: the Rust manager serving the built SPA (`atvprep manager api --web-dist dist`) on
 * http://127.0.0.1:8765, or PLAYWRIGHT_BASE_URL. The journeys need a seeded database and
 * E2E_MASTER_TOKEN; `scripts/run-e2e.sh` builds, seeds, starts the server and runs them.
 * The tests exercise the server-served SPA, NOT the Vite dev server.
 *
 * - Chromium only (the smoke just needs one browser).
 * - No local retries; CI may perform one visible retry.
 * - Trace + screenshot only on failure (keeps artifacts small).
 * - Video off.
 */

import { defineConfig, devices } from "@playwright/test";

const isCI = !!process.env.CI;

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  forbidOnly: !!isCI,
  retries: isCI ? 1 : 0,
  workers: 1,
  reporter: isCI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:8765",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "off",
    // SPA hydration: allow a bit more time than the default 5s in CI.
    actionTimeout: 10_000,
    navigationTimeout: 15_000,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});

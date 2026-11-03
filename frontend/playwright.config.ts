/**
 * Playwright config for the atvprep-manager SPA smoke test (Task 7).
 *
 * Target: the FastAPI server started by `scripts/run_manager_e2e.py`, which
 * installs the wheel into a temporary venv and boots uvicorn on
 * http://127.0.0.1:8765. The test exercises the *wheel-served* SPA, NOT the
 * Vite dev server.
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

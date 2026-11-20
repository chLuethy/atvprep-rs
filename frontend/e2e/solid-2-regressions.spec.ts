import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test("behält Dateiauswahl, DOM-Zeile und Fokus bei frischen Objekten und umgekehrter Reihenfolge", async ({ page }) => {
  let reads = 0;
  let release!: () => void;
  const ready = new Promise<void>((resolve) => { release = resolve; });
  await page.route("**/api/v1/dashboard", async (route) => {
    // APIRequestContext has no browser fetch metadata: send the actual page origin.
    const response = await route.fetch({ headers: { ...await route.request().allHeaders(), Origin: new URL(page.url()).origin } });
    if (!response.ok()) { await route.fulfill({ response }); return; }
    const body = await response.json();
    body.data.jobs = { running: 1 };
    await route.fulfill({ response, json: body });
  });
  await page.route(/\/api\/v1\/files\?/, async (route) => {
    const response = await route.fetch({ headers: { ...await route.request().allHeaders(), Origin: new URL(page.url()).origin } });
    if (!response.ok()) { await route.fulfill({ response }); return; }
    const body = await response.json();
    reads += 1;
    if (reads > 1) {
      await ready;
      body.data.reverse();
    }
    await route.fulfill({ response, json: body });
  });
  try {
    await openAndUnlock(page, "/files");
    const link = page.getByRole("link", { name: "Sample Movie", exact: true });
    await expect(link).toBeVisible();
    const titles = page.locator(".files-table .file-title");
    const before = await titles.allTextContents();
    const row = link.locator("xpath=ancestor::tr");
    const original = await row.elementHandle();
    await link.click({ modifiers: ["ControlOrMeta"] });
    await row.focus();
    await expect(row).toHaveAttribute("data-picked", "true");
    release();
    await expect.poll(() => reads).toBeGreaterThan(1);
    await expect(titles).toHaveText([...before].reverse());
    await expect(row).toHaveAttribute("data-picked", "true");
    await expect(row).toBeFocused();
    expect(await row.evaluate((node, original) => node === original, original)).toBe(true);
    await original?.dispose();
  } finally { release(); }
});

test("behält ein offenes Jobprotokoll bei wechselnden Job- und Item-IDs und fragt die aktuellen IDs ab", async ({ page }) => {
  let version = 1;
  const requestedLogs: string[] = [];
  const job = (id: string) => ({
    id, action: "verify", status: "running", trigger: "manual", origin_job_id: null,
    requested_at: "2026-11-14T08:00:00Z", started_at: "2026-11-14T08:01:00Z", finished_at: null,
    total_count: 1, succeeded_count: 0, failed_count: 0, cancelled_count: 0, subject: "Browser Identity.mp4",
  });
  const fulfill = (data: unknown) => ({ status: 200, json: { data, meta: {} } });
  await page.route(/\/api\/v1\/jobs\?/, (route) => route.fulfill(fulfill([job(`j_browser_${version}`)])));
  await page.route(/\/api\/v1\/jobs\/j_browser_[12](?:\/items\?.*)?$/, (route) => {
    const path = new URL(route.request().url()).pathname;
    const id = path.includes("j_browser_2") ? "2" : "1";
    return route.fulfill(fulfill(path.endsWith("/items") ? [{
      id: `i_browser_${id}`, job_id: `j_browser_${id}`, media_id: "m_browser", action: "verify", position: 1,
      status: "running", attempt: 1, output_media_id: null, display_path: "Browser Identity.mp4", error_code: null, error_detail: null,
    }] : job(`j_browser_${id}`)));
  });
  await page.route(/\/api\/v1\/job-items\/i_browser_[12]\/log$/, (route) => {
    const id = new URL(route.request().url()).pathname.includes("i_browser_2") ? "2" : "1";
    requestedLogs.push(id);
    return route.fulfill(fulfill({ log_tail: `Protokoll der aktuellen ID ${id}` }));
  });
  await openAndUnlock(page, "/jobs");
  await page.getByRole("region", { name: "Active" }).getByRole("link", { name: /Browser Identity/ }).click();
  const toggle = page.getByRole("button", { name: "Log", exact: true });
  await toggle.click();
  await expect(page.getByLabel("Plain text log")).toContainText("Protokoll der aktuellen ID 1");
  const original = await toggle.elementHandle();
  version = 2;
  await expect(page.getByLabel("Plain text log")).toContainText("Protokoll der aktuellen ID 2");
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(toggle).toBeFocused();
  expect(await toggle.evaluate((node, original) => node === original, original)).toBe(true);
  expect(requestedLogs).toContain("2");
  await expect(page.getByRole("region", { name: "Active" }).getByRole("link", { name: /Browser Identity/ })).toHaveAttribute("href", "/jobs/j_browser_2");
  await original?.dispose();
});

test("bewahrt während einer langsamen Speicherantwort weiterbearbeitete Einstellungen", async ({ page }) => {
  let submitted = false;
  let release!: () => void;
  const ready = new Promise<void>((resolve) => { release = resolve; });
  await page.route("**/api/v1/settings", async (route) => {
    if (route.request().method() !== "PATCH") { await route.continue(); return; }
    const response = await route.fetch();
    submitted = true;
    await ready;
    await route.fulfill({ response });
  });
  try {
    await openAndUnlock(page, "/settings");
    const interval = page.getByLabel("Scan interval (seconds)", { exact: true });
    await interval.fill("81");
    await page.getByRole("button", { name: "Save settings" }).click();
    await expect.poll(() => submitted).toBe(true);
    await interval.fill("91");
    await page.getByLabel("Parallel preparations", { exact: true }).fill("6");
    release();
    await expect(page.getByText("Settings saved.", { exact: true })).toBeVisible();
    await expect(interval).toHaveValue("91");
    await expect(page.getByLabel("Parallel preparations", { exact: true })).toHaveValue("6");
    await expect(page.getByText("Unsaved changes", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Save settings" })).toBeEnabled();
  } finally { release(); }
});

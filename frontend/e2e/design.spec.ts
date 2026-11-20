import { expect, test, type Page } from "@playwright/test";
import { contrastViolations, expectDesignHolds, narrowOverflow, openThemed } from "./design";

test.use({ trace: "off", screenshot: "off", video: "off" });

// Pages under the design check. Page tickets append their route here.
const PAGES: ReadonlyArray<{ name: string; path: string; ready: string; prepare?: (page: Page) => Promise<unknown> }> = [
  { name: "Overview", path: "/dashboard", ready: "Overview" },
  {
    name: "Schedules", path: "/schedules", ready: "Schedules",
    // Everything the page can show: the history of the seeded schedule, and the editor in cron mode with its guide.
    prepare: async (page) => {
      await page.getByRole("button", { name: /^History/ }).click();
      await page.getByRole("button", { name: /New schedule/ }).click();
      const editor = page.getByRole("region", { name: "New schedule" });
      await editor.getByRole("button", { name: "Cron", exact: true }).click();
      await editor.getByRole("button", { name: "Cron guide" }).click();
      await expect(editor.getByText("Examples. Click one to use it.")).toBeVisible();
    },
  },
  { name: "Settings", path: "/settings", ready: "Runtime settings" },
  // A failed job is open, so the list, the detail, its failure and the item row are all checked.
  {
    name: "Activity", path: "/jobs", ready: "Activity",
    // Job ids are opaque, so the failed job is opened from the list; the failure is in the page
    // before contrastViolations waits for what rises into place to settle.
    prepare: async (page) => {
      await page.getByRole("region", { name: "History" }).getByRole("link", { name: /Failed Movie Prepare/ }).click();
      await expect(page.getByRole("region", { name: "Why it failed" })).toBeVisible();
    },
  },
  { name: "Files", path: "/files", ready: "Files" },
  {
    // The columns view with a library open and two files picked, so the selection bar and the details show.
    name: "Files, columns view", path: "/files", ready: "Files",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Columns" }).click();
      const columns = page.getByRole("group", { name: /Files by folder/ });
      await columns.getByRole("listbox", { name: "Libraries" }).getByRole("option").first().click();
      await columns.getByRole("option", { name: /Failed Movie/ }).click();
      await columns.getByRole("option", { name: /Old Codec Movie/ }).click({ modifiers: ["ControlOrMeta"] });
      await expect(page.getByRole("region", { name: "Selection", exact: true })).toContainText(/\d+ selected/);
    },
  },
  {
    // The prepare preview for one file, with the encoding allowance choice.
    name: "Files, prepare preview", path: "/files", ready: "Files",
    prepare: async (page) => {
      await page.getByRole("link", { name: "Old Codec Movie" }).click();
      await page.getByRole("button", { name: /Preview and prepare/ }).click();
      await expect(page.getByRole("region", { name: "Prepare preview" })).toBeVisible();
    },
  },
];

for (const { name, path, ready, prepare } of PAGES) {
  test(`${name}: contrast in light and dark, no horizontal scroll at 560 px`, async ({ page }) => {
    await expectDesignHolds(page, path, { heading: ready, prepare });
  });
}

// The helper itself must be able to fail: each case plants a violation in the page.
test.describe("the design helper reports violations", () => {
  test("light grey text on white fails the contrast check", async ({ page }) => {
    await openThemed(page, "/dashboard", "light");
    await page.evaluate(() => {
      const el = document.createElement("p");
      el.id = "planted-contrast";
      el.textContent = "Planted low contrast";
      el.style.cssText = "color:#eeeeee;background:#ffffff;position:fixed;top:0;left:0;z-index:9999;margin:0";
      document.body.append(el);
    });
    const found = await contrastViolations(page);
    expect(found.join("\n")).toContain("planted-contrast");
  });

  test("a clean page has no contrast violations", async ({ page }) => {
    await openThemed(page, "/dashboard", "dark");
    expect(await contrastViolations(page)).toEqual([]);
  });

  test("an over-wide element is reported at 560 px", async ({ page }) => {
    await openThemed(page, "/dashboard", "light");
    await page.evaluate(() => {
      const el = document.createElement("div");
      el.style.cssText = "width:900px;height:4px";
      document.body.append(el);
    });
    expect(await narrowOverflow(page)).toBeGreaterThan(0);
  });
});

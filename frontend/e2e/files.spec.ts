import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("library journey", () => {
  test("filters, verifies a selection, deep-links the inspector and queues re-inspection", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await expect(page.getByRole("heading", { name: "Library" })).toBeVisible();
    await expect(page.getByRole("table", { name: "Media files" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();

    await page.getByLabel("Search files").fill("Sample Movie");
    const sampleLink = page.getByRole("link", { name: "Sample Movie" });
    await expect(sampleLink).toBeVisible();
    await expect(page.getByRole("link", { name: "Failed Movie" })).toHaveCount(0);

    // A click selects the file and opens it in the inspector, where Verify acts on it.
    await sampleLink.click();
    await expect(page).toHaveURL(/\/files\/.+/);
    await expect(page.getByRole("heading", { name: "Sample Movie" })).toBeVisible();
    await page.getByRole("button", { name: "Verify", exact: true }).click();
    await expect(page.locator(".lib-notice").getByText("Verification job queued", { exact: false })).toBeVisible();
    await expect(page.getByRole("list", { name: "Why" })).toHaveCount(0);

    await page.reload();
    await expect(page.getByRole("heading", { name: "Sample Movie" })).toBeVisible();

    await page.getByRole("button", { name: "Re-inspect" }).click();
    await expect(page.getByRole("region", { name: "Confirm re-inspection" })).toBeVisible();
    await page.getByRole("button", { name: "Confirm re-inspect" }).click();
    await expect(page.getByText("Re-inspection queued", { exact: true })).toBeVisible();
  });

  test("chosen properties show only files that have all of them, and nothing chosen shows everything", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();

    await page.getByRole("checkbox", { name: /^Needs re-encode/ }).check();
    await expect(page).toHaveURL(/has=reencode/);
    await expect(page.getByRole("link", { name: "Old Codec Movie" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Failed Movie" })).toHaveCount(0);

    // A second property narrows further: no file here has both.
    await page.getByRole("checkbox", { name: /^Embed open/ }).check();
    await expect(page).toHaveURL(/has=reencode%2Cembed/);
    await expect(page.getByRole("link", { name: "Old Codec Movie" })).toHaveCount(0);

    await page.getByRole("button", { name: "Show all files" }).click();
    await expect(page).not.toHaveURL(/has=/);
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Old Codec Movie" })).toBeVisible();
  });

  test("shows the scope of a prepare run before anything is queued", async ({ page }) => {
    await openAndUnlock(page, "/files");

    await expect(page.getByRole("button", { name: "Prepare All" })).toHaveCount(0);
    await page.getByRole("link", { name: "Failed Movie" }).click();
    await page.getByRole("button", { name: /Prepare preview/ }).click();

    const preview = page.getByRole("region", { name: "Prepare preview" });
    await expect(preview).toContainText("Failed Movie");
    await expect(preview).toContainText("Nothing is written until you confirm.");
    await expect(page.getByRole("button", { name: "Prepare 1 file" })).toBeVisible();

    await preview.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByRole("region", { name: "Prepare preview" })).toHaveCount(0);
  });

  test("the preview names the files that need a re-encode and offers both ways", async ({ page }) => {
    await openAndUnlock(page, "/files");

    await page.getByRole("link", { name: "Old Codec Movie" }).click();
    await page.getByRole("button", { name: /Prepare preview/ }).click();

    const preview = page.getByRole("region", { name: "Prepare preview" });
    await expect(preview).toContainText("Old Codec Movie");
    await expect(preview).toContainText("1 of 1 file needs a re-encode");
    await expect(preview.getByRole("button", { name: "With re-encode (1)" })).toBeEnabled();
    // The only file needs a re-encode, so leaving it out would queue nothing.
    await expect(preview.getByRole("button", { name: "Without re-encode" })).toBeDisabled();
    await expect(preview.getByRole("button", { name: "Prepare 1 file" })).toHaveCount(0);

    await preview.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByRole("region", { name: "Prepare preview" })).toHaveCount(0);
  });

  test("flags a file whose preparation was interrupted and prepares it again only by hand", async ({ page }) => {
    await openAndUnlock(page, "/files");

    const row = page.getByRole("link", { name: "Interrupted Movie" }).locator("xpath=ancestor::tr");
    await expect(row).toContainText("Last preparation interrupted");
    await expect(page.getByRole("link", { name: "Failed Movie" }).locator("xpath=ancestor::tr")).not.toContainText("Last preparation interrupted");

    await page.getByRole("link", { name: "Interrupted Movie" }).click();
    await expect(page.getByRole("heading", { name: "Interrupted Movie" })).toBeVisible();
    await expect(page.getByRole("status").filter({ hasText: "Last preparation interrupted" })).toBeVisible();

    // Nothing is queued on its own; the button opens the normal preview with its confirmation.
    await page.getByRole("button", { name: "Prepare again" }).click();
    const preview = page.getByRole("region", { name: "Prepare preview" });
    await expect(preview).toContainText("Nothing is written until you confirm.");
    await preview.getByRole("button", { name: "Cancel" }).click();
  });
});

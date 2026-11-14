import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("files journey", () => {
  test("filters, verifies a selection, deep-links the inspector and queues re-inspection", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await expect(page.getByRole("heading", { name: "Files" })).toBeVisible();
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

  test("the filter bar narrows by state (any) and property (all), keeps it in the address and Back restores it", async ({ page }) => {
    await openAndUnlock(page, "/files");
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();
    const bar = page.getByRole("search", { name: "Filter files" });
    await expect(bar.getByText("4 of 4 files")).toBeVisible();

    await bar.getByRole("button", { name: "Status", exact: true }).click();
    const status = page.getByRole("dialog", { name: "Status" });
    await status.getByRole("checkbox", { name: /^Ready/ }).check();
    await expect(page).toHaveURL(/\?status=clean$/);
    await expect(page.getByRole("link", { name: "Failed Movie" })).toHaveCount(0);
    await expect(page.getByRole("link", { name: "Sample Movie" })).toBeVisible();
    await expect(bar.getByText("1 of 4 files")).toBeVisible();

    // A second state adds its files (OR).
    await status.getByRole("checkbox", { name: /^Warning/ }).check();
    await expect(page).toHaveURL(/\?status=clean,warning$/);
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(status).toHaveCount(0);
    await expect(bar.getByRole("button", { name: "Status: Ready, Warning" })).toBeFocused();

    // A property narrows (AND).
    await bar.getByRole("button", { name: "Properties", exact: true }).click();
    await page.getByRole("dialog", { name: "Properties" }).getByRole("checkbox", { name: /^Needs video encoding/ }).check();
    await expect(page).toHaveURL(/\?status=clean,warning&has=encode_video$/);
    await expect(page.getByRole("link", { name: "Old Codec Movie" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Failed Movie" })).toHaveCount(0);

    // Back undoes one choice at a time.
    await page.goBack();
    await expect(page).toHaveURL(/\?status=clean,warning$/);
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();
    await expect(bar.getByRole("button", { name: "Properties", exact: true })).toBeVisible();
    await page.goBack();
    await expect(page).toHaveURL(/\?status=clean$/);
    await expect(page.getByRole("link", { name: "Failed Movie" })).toHaveCount(0);

    await bar.getByRole("button", { name: "Reset filters" }).click();
    await expect(page).toHaveURL(/\/files$/);
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Sample Movie" })).toBeVisible();
  });

  test("an old address with hide shows the same files as before, under the new address", async ({ page }) => {
    await openAndUnlock(page, "/files?hide=clean");
    await expect(page).toHaveURL(/\?status=embed_open,warning,blocker,unknown,probe_failed,missing$/);
    await expect(page.getByRole("link", { name: "Failed Movie" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Sample Movie" })).toHaveCount(0);
  });

  test("on a narrow screen the facets scroll in one row and a popover takes the whole screen", async ({ page }) => {
    await openAndUnlock(page, "/files?has=encode_video");
    await page.setViewportSize({ width: 390, height: 844 });
    const bar = page.getByRole("search", { name: "Filter files" });
    await expect(bar.getByRole("button", { name: "All filters (1 active)" })).toBeVisible();
    // The active facet comes right after the filter symbol.
    const facets = bar.locator(".fb-facets > .fb-facet");
    const left = async (name: string) => (await bar.getByRole("button", { name, exact: true }).boundingBox())!.x;
    expect(await left("Properties: Needs video encoding")).toBeLessThan(await left("Status"));
    await expect(facets.first()).toBeVisible();
    // Search, view switch and the facet row fit the width; only the facet row scrolls.
    expect(await bar.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);

    await bar.getByRole("button", { name: "Status", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Status" });
    const box = (await dialog.boundingBox())!;
    expect(box.width).toBeGreaterThanOrEqual(389);
    expect(box.height).toBeGreaterThanOrEqual(843);
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toHaveCount(0);
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

  test("the preview offers the encoding allowance and holds the video level back until it is confirmed", async ({ page }) => {
    await openAndUnlock(page, "/files");

    await page.getByRole("link", { name: "Old Codec Movie" }).click();
    await page.getByRole("button", { name: /Prepare preview/ }).click();

    const preview = page.getByRole("region", { name: "Prepare preview" });
    await expect(preview).toContainText("Old Codec Movie");
    await expect(preview).toContainText("1 of 1 file needs a re-encode");
    await expect(preview.getByRole("radio", { name: /^Encode audio(?! and)/ })).toBeChecked();
    const prepare = preview.getByRole("button", { name: "Prepare 1 file" });
    await expect(prepare).toBeEnabled();

    await preview.getByRole("radio", { name: /^Encode audio and video/ }).check();
    await expect(prepare).toBeDisabled();
    await preview.getByRole("checkbox", { name: /re-encoded/ }).check();
    await expect(prepare).toBeEnabled();

    // The real server takes the confirmed level; the workers are parked, so the file stays queued.
    await prepare.click();
    await expect(page.getByText("Prepare job queued for 1 file. Follow it under Activity.")).toBeVisible();
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

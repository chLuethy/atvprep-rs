import { expect, test } from "@playwright/test";
import { openAndUnlock } from "./support";

test.use({ trace: "off", screenshot: "off", video: "off" });

test.describe("schedules journey", () => {
  test("a schedule that prepares starts paused at Copy only, and turning it on names what is signed", async ({ page }) => {
    await openAndUnlock(page, "/schedules");
    await page.getByRole("button", { name: /New schedule/ }).click();
    const editor = page.getByRole("region", { name: "New schedule" });
    await editor.getByLabel("Name").fill("Nightly prepare");
    await editor.getByRole("group", { name: "Action" }).getByRole("button", { name: "Prepare" }).click();
    await expect(editor.getByRole("radio", { name: "Copy only" })).toBeChecked();
    await editor.getByRole("radio", { name: "Encode audio", exact: true }).check();
    await editor.getByLabel("Files per run").fill("5");
    await editor.getByRole("button", { name: "Save schedule" }).click();
    await expect(page.getByText("Schedule saved. It is paused until you turn it on.")).toBeVisible();

    const list = page.getByRole("list", { name: "Schedules" });
    await expect(list).toContainText("Prepare · All libraries · up to 5 files · Encode audio");
    await page.getByRole("checkbox", { name: "Turn on Nightly prepare" }).click();
    const question = page.getByRole("region", { name: "Turn on Nightly prepare" });
    await expect(question).toContainText("Encoding allowance: Encode audio.");
    await expect(question).toContainText("Target profile: CRF 18, HEVC10 off, keep AV1 off, DTS fix off.");
    await expect(question).toContainText(/files? match(es)? now/);
    await question.getByRole("button", { name: "Confirm turn on" }).click();
    await expect(page.getByText("Schedule is on.")).toBeVisible();
    await expect(page.getByRole("checkbox", { name: "Pause Nightly prepare" })).toBeChecked();
  });
});

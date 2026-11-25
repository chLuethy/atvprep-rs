import { expect, test } from "@playwright/test";
import { openThemed } from "./design";

test("Alte Naturthemen werden verworfen; ausschließlich Kupfer bleibt verfügbar", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("atvprep-palette", "forest"));
  await openThemed(page, "/settings", "light", "Runtime settings");
  const appearance = page.getByRole("region", { name: /^Appearance:?$/ });
  await expect(appearance.getByRole("radio")).toHaveCount(0);
  await expect(page.locator("html")).not.toHaveAttribute("data-palette");
  expect(await page.evaluate(() => localStorage.getItem("atvprep-palette"))).toBeNull();
  const accent = () => page.locator('.nav-link[aria-current="page"]').evaluate((element) => getComputedStyle(element).color);
  await expect.poll(accent).toBe("rgb(128, 92, 58)");
  await page.getByRole("group", { name: "Color mode", exact: true }).getByRole("button", { name: "Dark", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect.poll(accent).toBe("rgb(201, 157, 116)");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.locator("html")).not.toHaveAttribute("data-palette");
  expect(await page.evaluate(() => localStorage.getItem("atvprep-palette"))).toBeNull();
});

test("Moduswahl bleibt mit der Sidebar synchron und bewahrt offene Einstellungsentwürfe", async ({ page }) => {
  await openThemed(page, "/settings", "light", "Runtime settings");
  const crf = page.getByLabel("CRF", { exact: true });
  await expect(crf).toBeVisible();
  const next = Number(await crf.inputValue()) === 21 ? "22" : "21";
  await crf.fill(next);
  const modes = page.getByRole("group", { name: "Color mode", exact: true });
  for (const name of ["Dark", "System", "Light"]) {
    await modes.getByRole("button", { name, exact: true }).click();
    await expect(page.locator(".sidebar-foot").getByRole("button", { name, exact: true })).toHaveAttribute("aria-pressed", "true");
    await expect(crf).toHaveValue(next);
    await expect(page.getByRole("button", { name: "Save settings", exact: true })).toBeEnabled();
  }
});

import { expect, type Page } from "@playwright/test";

export async function unlock(page: Page): Promise<void> {
  await page.getByLabel("Master token").fill(process.env.E2E_MASTER_TOKEN ?? "");
  await page.getByRole("button", { name: "Unlock" }).click();
  await expect(page.getByRole("button", { name: "Lock", exact: true })).toBeVisible();
}

export async function openAndUnlock(page: Page, path: string): Promise<void> {
  // The one-time keyboard tip floats over the bottom right corner; a journey is not about it.
  await page.addInitScript(() => window.localStorage.setItem("atvprep:tip:keys", "seen"));
  await page.goto(path);
  await unlock(page);
}

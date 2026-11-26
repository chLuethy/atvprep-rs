import { expect, type Page } from "@playwright/test";

export async function unlock(page: Page): Promise<void> {
  const lock = page.getByRole("button", { name: "Lock", exact: true });
  // The gate shows the token form while it still asks the server whether a session exists; with a session (a
  // reload, a second page in the same context) the form goes away by itself. Do not wait for a form that left.
  const unlocked = await page.getByLabel("Master token").fill(process.env.E2E_MASTER_TOKEN ?? "", { timeout: 2_000 }).then(() => false, () => lock.isVisible());
  if (!unlocked) {
    await page.getByRole("button", { name: "Unlock" }).click({ timeout: 2_000 }).catch(async (error) => { if (!(await lock.isVisible())) throw error; });
  }
  await expect(lock).toBeVisible();
}

export async function openAndUnlock(page: Page, path: string): Promise<void> {
  // The one-time keyboard tip floats over the bottom right corner; a journey is not about it.
  await page.addInitScript(() => window.localStorage.setItem("atvprep:tip:keys", "seen"));
  await page.goto(path);
  await unlock(page);
}

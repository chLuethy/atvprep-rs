/**
 * Design check for the journeys: colour contrast in light and dark (axe, real browser; jsdom
 * cannot measure contrast) and no horizontal scrolling at 560 px width.
 *
 * Attach a page: add `{ name, path, ready }` to PAGES in `design.spec.ts`. `ready` is the
 * heading text that shows the page has rendered. For a page that needs more than a heading
 * (data loaded, a dialog open), pass `prepare` to `expectDesignHolds`:
 *
 *   await expectDesignHolds(page, "/files", { heading: "Files", prepare: (p) => p.getByRole("table").waitFor() });
 *
 * Building blocks, if a page needs a custom journey: `openThemed` (open + unlock in a theme),
 * `contrastViolations` (axe colour-contrast findings as readable lines), `narrowOverflow`
 * (pixels the page is wider than a 560 px window; 0 means no horizontal scroll).
 */

import { createRequire } from "node:module";
import { expect, type Page } from "@playwright/test";
import { unlock } from "./support";

export type Theme = "light" | "dark";

export const NARROW_WIDTH = 560;

const AXE_SOURCE = createRequire(import.meta.url).resolve("axe-core/axe.min.js");

/** Open `path` with the stored theme choice set, unlock, and wait for the page heading. */
export async function openThemed(page: Page, path: string, theme: Theme, heading?: string): Promise<void> {
  // Storage is set on the page itself and the page reloaded, so a second call on the same page
  // (another theme) is not overruled by an init script of the first. The one-time keyboard tip
  // floats over the page; the design check is not about it.
  await page.goto(path);
  const lock = page.getByRole("button", { name: "Lock", exact: true });
  const tokenField = page.getByLabel("Master token");
  await expect(lock.or(tokenField)).toBeVisible();
  if (await tokenField.isVisible()) await unlock(page);
  await page.evaluate((value) => {
    window.localStorage.setItem("atvprep:tip:keys", "seen");
    window.localStorage.setItem("atvprep-theme", value);
  }, theme);
  await page.reload();
  await expect(lock).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
  if (heading) await expect(page.getByRole("heading", { name: heading })).toBeVisible();
}

/** Colour-contrast findings of axe for the current page, one readable line per element. */
export async function contrastViolations(page: Page): Promise<string[]> {
  await page.addScriptTag({ path: AXE_SOURCE });
  const results = await page.evaluate(async () => {
    const axe = (window as unknown as { axe: { run: (ctx: Document, opts: object) => Promise<{ violations: Array<{ nodes: Array<{ target: unknown[]; any: Array<{ message: string }> }> }> }> } }).axe;
    return axe.run(document, { runOnly: { type: "rule", values: ["color-contrast"] } });
  });
  return results.violations.flatMap((violation) =>
    violation.nodes.map((node) => `${node.target.join(" ")}: ${node.any[0]?.message ?? "contrast too low"}`),
  );
}

/** How many pixels the page is wider than a 560 px window (0: no horizontal scroll). */
export async function narrowOverflow(page: Page): Promise<number> {
  await page.setViewportSize({ width: NARROW_WIDTH, height: 900 });
  return page.evaluate(() => {
    const root = document.documentElement;
    return Math.max(0, Math.max(root.scrollWidth, document.body.scrollWidth) - root.clientWidth);
  });
}

/** The whole check for one page: contrast in light and dark, then no overflow at 560 px. */
export async function expectDesignHolds(
  page: Page,
  path: string,
  options: { heading: string; prepare?: (page: Page) => Promise<unknown> },
): Promise<void> {
  for (const theme of ["light", "dark"] as const) {
    await openThemed(page, path, theme, options.heading);
    await options.prepare?.(page);
    expect(await contrastViolations(page), `contrast in ${theme}`).toEqual([]);
  }
  expect(await narrowOverflow(page), `horizontal overflow at ${NARROW_WIDTH}px`).toBe(0);
}

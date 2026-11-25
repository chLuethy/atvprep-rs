import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { openThemed, settled } from "./design";

const mockup = readFileSync(new URL("../../docs/design/manager-mockup.html", import.meta.url), "utf8");
const referenceStyle = /<style>([\s\S]*?)<\/style>/.exec(mockup)![1];
const routes = ["/files", "/jobs", "/schedules", "/dashboard", "/settings"];

test.use({ trace: "off", screenshot: "off", video: "off" });

for (const theme of ["light", "dark"] as const) {
  test(`Typografie, Palette und Raster entsprechen dem Mockup (${theme})`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await openThemed(page, "/files", theme, "Files");
    const reference = await page.context().newPage();
    await reference.setViewportSize({ width: 1440, height: 900 });
    await reference.setContent(`<style>${referenceStyle}</style><div class="app"><aside class="side"><span class="label">Darstellung:</span></aside><main class="main"><div class="chl-titlebar">Dateien</div></main></div>`);
    await reference.evaluate((value) => document.documentElement.dataset.theme = value, theme);
    const expected = await reference.evaluate(() => ({
      bodySize: getComputedStyle(document.body).fontSize,
      bodyLeading: getComputedStyle(document.body).lineHeight,
      sidebarWidth: document.querySelector(".side")!.getBoundingClientRect().width,
      gutter: getComputedStyle(document.querySelector(".main")!).paddingLeft,
      titleSize: getComputedStyle(document.querySelector(".chl-titlebar")!).fontSize,
      titleWeight: getComputedStyle(document.querySelector(".chl-titlebar")!).fontWeight,
      palette: ["--surface", "--surface-raised", "--brand", "--on-brand", "--ink", "--ink-muted", "--sand", "--line"].map((token) => {
        const probe = document.createElement("span");
        probe.style.color = `var(${token})`;
        document.body.append(probe);
        const color = getComputedStyle(probe).color;
        probe.remove();
        return color;
      }),
    }));
    await reference.close();
    const actual = await page.evaluate(() => ({
      bodySize: getComputedStyle(document.body).fontSize,
      bodyLeading: getComputedStyle(document.body).lineHeight,
      sidebarWidth: document.querySelector(".sidebar")!.getBoundingClientRect().width,
      gutter: getComputedStyle(document.querySelector(".page")!).paddingLeft,
      titleSize: getComputedStyle(document.querySelector(".page-titlebar")!).fontSize,
      titleWeight: getComputedStyle(document.querySelector(".page-titlebar")!).fontWeight,
      palette: ["--ground", "--surface", "--accent", "--on-accent", "--ink", "--ink-muted", "--sand", "--line"].map((token) => {
        const probe = document.createElement("span");
        probe.style.color = `var(${token})`;
        document.body.append(probe);
        const color = getComputedStyle(probe).color;
        probe.remove();
        return color;
      }),
    }));
    // Dunkle Schriftflächen behalten die bereits vorhandene, etwas größere Zeilenhöhe.
    expect({ ...actual, bodyLeading: expected.bodyLeading }).toEqual(expected);
    expect(Number.parseFloat(actual.bodyLeading)).toBeGreaterThanOrEqual(Number.parseFloat(expected.bodyLeading));
    const geometry = await page.locator(".foot-appearance").evaluate((element) => {
      const frame = element.getBoundingClientRect();
      const group = element.querySelector(".seg")!.getBoundingClientRect();
      return { width: frame.width, groupWidth: group.width, choices: [...element.querySelectorAll("button")].map((button) => button.getBoundingClientRect().width) };
    });
    expect(geometry.groupWidth).toBe(geometry.width);
    expect(Math.max(...geometry.choices) - Math.min(...geometry.choices)).toBeLessThan(1);
  });

  for (const language of ["en", "de"] as const) {
    test(`Alle Seiten bleiben ausgerichtet und innerhalb des Viewports (${theme}, ${language})`, async ({ page }) => {
      test.setTimeout(120_000);
      await page.setViewportSize({ width: 1440, height: 900 });
      await openThemed(page, "/files", theme, "Files");
      await page.locator(`.sidebar-foot button[lang="${language}"]`).click();
      for (const width of [360, 768, 1024, 1440, 1920]) {
        await page.setViewportSize({ width, height: 900 });
        for (const route of routes) {
          await page.goto(route);
          const title = page.locator(".page-titlebar").first();
          await expect(title).toBeVisible();
          await expect(page.locator('.nav-link[aria-current="page"]')).toHaveAttribute("href", route);
          await settled(page);
          const geometry = await page.locator(".page").evaluate((element) => {
            const style = getComputedStyle(element);
            const title = element.querySelector(".page-titlebar")!;
            const heading = getComputedStyle(title);
            const bounds = title.getBoundingClientRect();
            const root = document.documentElement;
            return {
              inset: Number.parseFloat(style.paddingLeft),
              overflow: Math.max(root.scrollWidth, document.body.scrollWidth) - root.clientWidth,
              contentOverflow: element.scrollWidth - element.clientWidth,
              titleWidth: bounds.width,
              titleRight: bounds.right,
              titleBorder: heading.borderTopWidth,
              titleSize: heading.fontSize,
            };
          });
          expect(geometry.inset, `${route}, ${width}px`).toBe(width <= 860 ? 16 : 32);
          expect(geometry.overflow, `${route}, ${width}px`).toBeLessThanOrEqual(1);
          expect(geometry.contentOverflow, `${route}, ${width}px`).toBeLessThanOrEqual(1);
          expect(geometry.titleWidth).toBeLessThanOrEqual(1360);
          expect(geometry.titleRight).toBeLessThanOrEqual(width);
          expect(geometry.titleBorder).toBe("1px");
          expect(geometry.titleSize).toBe("20px");
          if (process.env.ATVPREP_DESIGN_SCREENSHOTS && (width === 360 || width === 1440)) {
            await page.screenshot({ path: `${process.env.ATVPREP_DESIGN_SCREENSHOTS}/${route.slice(1)}-${width}-${theme}-${language}.png` });
          }
        }
      }
    });
  }
}

test("Das schmale Navigationsmenü hält Darstellung und Sprache unter den Navigationszielen", async ({ page }) => {
  await openThemed(page, "/files", "light", "Files");
  await page.setViewportSize({ width: 360, height: 900 });
  await page.getByRole("button", { name: "Menu", exact: true }).click();
  const appearance = page.getByRole("group", { name: "Appearance" });
  const language = page.getByRole("group", { name: "Language" });
  await expect(appearance).toBeVisible();
  await expect(language).toBeVisible();
  const first = (await appearance.boundingBox())!;
  const second = (await language.boundingBox())!;
  expect(second.y).toBeGreaterThanOrEqual(first.y + first.height);
  expect(first.x).toBe(second.x);
  expect(first.width).toBe(second.width);
});

test.describe("Touch-Bedienung", () => {
  test.use({ hasTouch: true });

  for (const width of [360, 1024]) {
    test(`Bedienelemente haben auf allen Seiten mindestens 44px Höhe bei ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await openThemed(page, "/files", "light", "Files");
      await page.setViewportSize({ width, height: 900 });
      expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
      for (const route of routes) {
        await page.goto(route);
        await expect(page.locator(".page-titlebar").first()).toBeVisible();
        await settled(page);
        const tooSmall = await page.locator(".page .btn, .page .seg button, .page .input, .page .select, .page .fb-button, .page .fb-link, .nav-link, .menu-toggle").evaluateAll((elements) =>
          elements.filter((element) => element.getClientRects().length > 0 && element.getBoundingClientRect().height < 44)
            .map((element) => ({ text: element.textContent, class: element.className, height: element.getBoundingClientRect().height })),
        );
        expect(tooSmall, route).toEqual([]);
      }
    });
  }
});

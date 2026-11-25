import type { Component } from "solid-js";
import { t } from "../../lib/i18n";
import { ThemeSwitch } from "../../ui/ThemeSwitch";

export const AppearanceSettings: Component = () => (
  <section class="setting-group appearance-settings" aria-labelledby="appearance-title">
    <h3 id="appearance-title">{t("Appearance")}</h3>
    <p class="help">{t("Applies immediately to all pages. Stored in this browser, not on the server.")}</p>
    <div class="appearance-mode">
      <span class="label">{t("Color mode")}</span>
      <ThemeSwitch label={t("Color mode")} fitBottom={false} />
    </div>
  </section>
);

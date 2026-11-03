import { For, type Component } from "solid-js";
import { t } from "../../lib/i18n";
import { SHORTCUTS } from "./shortcuts";

/** The shortcut list on its own; the page that shows it supplies the frame and the name. */
export const ShortcutsPanel: Component = () => (
  <div class="shortcuts">
    <For each={SHORTCUTS}>{(group) => (
      <section role="group" aria-label={t(group.title)}>
        <h3>{t(group.title)}</h3>
        <dl>
          <For each={group.items}>{(item) => (
            <div>
              <dt>
                <For each={item.chords}>{(chord, index) => (
                  <>
                    {index() > 0 ? <span class="muted"> {t("or")} </span> : null}
                    <For each={chord}>{(key) => <kbd class="kbd">{key}</kbd>}</For>
                  </>
                )}</For>
              </dt>
              <dd>{t(item.does)}</dd>
            </div>
          )}</For>
        </dl>
      </section>
    )}</For>
  </div>
);

import { t } from "../lib/i18n";
import { createEffect, createMemo, createSignal, For, Show, type Component } from "solid-js";

export type Command = { id: string; label: string; hint?: string; run: () => void };

/**
 * A keyboard-first command list (⌘K). The search owns focus the whole time; arrows move a
 * highlight, Enter runs it, Escape leaves. Focus goes back to wherever it was when it opened.
 */
export const CommandPalette: Component<{ open: boolean; commands: Command[]; onClose: () => void }> = (props) => {
  const [query, setQuery] = createSignal("");
  const [index, setIndex] = createSignal(0);
  let input: HTMLInputElement | undefined;
  let previous: Element | null = null;

  /** 0: the label starts with the query, 1: a word does, 2: it is only somewhere inside. */
  const rank = (label: string, needle: string) => {
    const text = label.toLowerCase();
    if (text.startsWith(needle)) return 0;
    if (text.split(/[^a-z0-9]+/).some((word) => word.startsWith(needle))) return 1;
    return text.includes(needle) ? 2 : -1;
  };
  const visible = createMemo(() => {
    const needle = query().trim().toLowerCase();
    if (!needle) return props.commands;
    return props.commands
      .map((command, position) => ({ command, position, score: rank(command.label, needle) }))
      .filter((entry) => entry.score >= 0)
      .sort((a, b) => a.score - b.score || a.position - b.position)
      .map((entry) => entry.command);
  });

  createEffect(() => {
    if (props.open) {
      previous = document.activeElement;
      setQuery("");
      setIndex(0);
      queueMicrotask(() => input?.focus());
    } else if (previous instanceof HTMLElement) {
      const target = previous;
      previous = null;
      queueMicrotask(() => target.focus());
    }
  });

  const run = (command: Command | undefined) => {
    if (!command) return;
    props.onClose();
    command.run();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    const last = Math.max(0, visible().length - 1);
    if (event.key === "ArrowDown") { event.preventDefault(); setIndex((value) => Math.min(last, value + 1)); }
    else if (event.key === "ArrowUp") { event.preventDefault(); setIndex((value) => Math.max(0, value - 1)); }
    else if (event.key === "Enter") { event.preventDefault(); run(visible()[index()]); }
    else if (event.key === "Escape") { event.preventDefault(); props.onClose(); }
    else if (event.key === "Tab") event.preventDefault();
  };

  return (
    <Show when={props.open}>
      <div class="pal-bg" onClick={(event) => { if (event.target === event.currentTarget) props.onClose(); }}>
        <div class="pal" role="dialog" aria-modal="true" aria-label={t("Command palette")}>
          <input
            ref={input}
            class="pal-input"
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-activedescendant={visible()[index()] ? `palette-${visible()[index()]!.id}` : undefined}
            aria-label={t("Search commands")}
            autocomplete="off"
            placeholder={t("Type a command")}
            value={query()}
            onInput={(event) => { setQuery(event.currentTarget.value); setIndex(0); }}
            onKeyDown={onKeyDown}
          />
          <ul id="palette-list" role="listbox" class="pal-list">
            <For each={visible()} fallback={<li class="pal-empty">{t("No command found")}</li>}>{(command, position) => (
              <li
                id={`palette-${command.id}`}
                role="option"
                aria-selected={index() === position()}
                onMouseMove={() => setIndex(position())}
                onClick={() => run(command)}
              >
                <span>{command.label}</span>
                <Show when={command.hint}><span class="pal-hint">{command.hint}</span></Show>
              </li>
            )}</For>
          </ul>
        </div>
      </div>
    </Show>
  );
};

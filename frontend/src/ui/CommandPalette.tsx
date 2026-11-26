import { t } from "../lib/i18n";
import { createEffect, createMemo, createSignal, For, Show, type Component } from "solid-js";

export type Command = { id: string; label: string; hint?: string; run: () => void };

/** The pause after the last keystroke before the search asks the server. */
const SEARCH_DELAY_MS = 200;

/** `label` cut into the parts around the first (case-insensitive) occurrence of `needle`; no occurrence is one plain part. */
function marked(label: string, needle: string): { before: string; hit: string; after: string } | null {
  const at = needle ? label.toLowerCase().indexOf(needle.toLowerCase()) : -1;
  return at < 0 ? null : { before: label.slice(0, at), hit: label.slice(at, at + needle.length), after: label.slice(at + needle.length) };
}

/**
 * A keyboard-first command list (⌘K). The search owns focus the whole time; arrows move a
 * highlight, Enter runs it, Escape leaves. Focus goes back to wherever it was when it opened.
 */
export const CommandPalette: Component<{
  open: boolean;
  commands: Command[];
  /** More hits for a typed query (files, jobs, schedules), asked after a pause; the commands work without it. */
  search?: (query: string) => Promise<Command[]>;
  onClose: () => void;
}> = (props) => {
  const [query, setQuery] = createSignal("");
  const [found, setFound] = createSignal<Command[]>([]);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let asked = 0;

  const ask = (text: string) => {
    clearTimeout(timer);
    const turn = ++asked;
    setFound([]);
    const needle = text.trim();
    if (!props.search || !needle) return;
    timer = setTimeout(() => {
      props.search?.(needle).then((hits) => { if (turn === asked) setFound(hits); }, () => undefined);
    }, SEARCH_DELAY_MS);
  };
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
    // Hits come after commands of the same weight; a hit the server found by its folder only is weakest.
    const commands = props.commands.map((command) => ({ command, score: rank(command.label, needle), hit: 0 }));
    const hits = found().map((command) => ({ command, score: rank(command.label, needle) >= 0 ? rank(command.label, needle) : 3, hit: 1 }));
    return [...commands.filter((entry) => entry.score >= 0), ...hits]
      .map((entry, position) => ({ ...entry, position }))
      .sort((a, b) => a.score - b.score || a.hit - b.hit || a.position - b.position)
      .map((entry) => entry.command);
  });

  createEffect(() => props.open, (open) => {
    ask("");
    if (open) {
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
            onInput={(event) => { setQuery(event.currentTarget.value); setIndex(0); ask(event.currentTarget.value); }}
            onKeyDown={onKeyDown}
          />
          <ul id="palette-list" role="listbox" class="pal-list">
            <For each={visible()} fallback={<li class="pal-empty">{t("No command found")}</li>}>{(command, position) => (
              <li
                id={`palette-${command.id}`}
                role="option"
                aria-label={command.hint ? `${command.label}, ${command.hint}` : command.label}
                aria-selected={index() === position() ? "true" : "false"}
                onMouseMove={() => setIndex(position())}
                onClick={() => run(command)}
              >
                <span class="pal-label" title={command.label}>
                  {(() => {
                    const parts = marked(command.label, query().trim());
                    return parts ? <>{parts.before}<mark>{parts.hit}</mark>{parts.after}</> : command.label;
                  })()}
                </span>
                <Show when={command.hint}><span class="pal-hint">{command.hint}</span></Show>
              </li>
            )}</For>
          </ul>
        </div>
      </div>
    </Show>
  );
};

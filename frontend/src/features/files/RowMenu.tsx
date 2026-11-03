import { createSignal, For, onCleanup, onMount, Show, type Component } from "solid-js";
import { Portal } from "solid-js/web";

export type MenuItem = {
  id: string;
  label: string;
  /** A key that does the same without the menu. */
  hint?: string;
  disabled?: boolean;
  /** A thin line above this item, to group what belongs together. */
  separated?: boolean;
  run: () => void;
};

/**
 * A context menu at the pointer. It closes on Escape, on a click elsewhere, on scrolling and on
 * resizing; arrow keys move through it and Enter chooses. Focus goes back to whoever opened it.
 */
export const RowMenu: Component<{ x: number; y: number; label: string; items: MenuItem[]; onClose: () => void }> = (props) => {
  let menu: HTMLDivElement | undefined;
  const opener = document.activeElement;
  const [position, setPosition] = createSignal({ left: props.x, top: props.y });

  const buttons = () => Array.from(menu?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
  const close = () => {
    props.onClose();
    if (opener instanceof HTMLElement) queueMicrotask(() => opener.focus());
  };

  onMount(() => {
    // Keep the menu on screen: flip it to the other side of the pointer when it would run off.
    const rect = menu?.getBoundingClientRect();
    if (rect) {
      setPosition({
        left: Math.max(4, Math.min(props.x, window.innerWidth - rect.width - 4)),
        top: Math.max(4, Math.min(props.y, window.innerHeight - rect.height - 4)),
      });
    }
    buttons()[0]?.focus();
    const outside = (event: Event) => { if (menu && !menu.contains(event.target as Node)) props.onClose(); };
    const away = () => props.onClose();
    // Opening the menu may itself scroll the columns (the row it acts on is brought into view); that
    // scrolling is not the person moving on.
    const openedAt = performance.now();
    const scrolled = () => { if (performance.now() - openedAt > 700) props.onClose(); };
    document.addEventListener("mousedown", outside, true);
    document.addEventListener("contextmenu", outside, true);
    window.addEventListener("resize", away);
    window.addEventListener("blur", away);
    document.addEventListener("scroll", scrolled, true);
    onCleanup(() => {
      document.removeEventListener("mousedown", outside, true);
      document.removeEventListener("contextmenu", outside, true);
      window.removeEventListener("resize", away);
      window.removeEventListener("blur", away);
      document.removeEventListener("scroll", scrolled, true);
    });
  });

  const onKeyDown = (event: KeyboardEvent) => {
    const all = buttons();
    const at = all.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
    else if (event.key === "ArrowDown") { event.preventDefault(); all[(at + 1) % all.length]?.focus(); }
    else if (event.key === "ArrowUp") { event.preventDefault(); all[(at - 1 + all.length) % all.length]?.focus(); }
    else if (event.key === "Home") { event.preventDefault(); all[0]?.focus(); }
    else if (event.key === "End") { event.preventDefault(); all[all.length - 1]?.focus(); }
    else if (event.key === "Tab") { event.preventDefault(); close(); }
  };

  return (
    <Portal>
      <div ref={menu} class="row-menu" role="menu" aria-label={props.label} style={`left: ${position().left}px; top: ${position().top}px`} onKeyDown={onKeyDown}>
        <For each={props.items}>{(item) => (
          <>
            <Show when={item.separated}><div class="row-menu-sep" role="separator" /></Show>
            <button type="button" role="menuitem" class="row-menu-item" disabled={item.disabled} onClick={() => { props.onClose(); item.run(); }}>
              <span>{item.label}</span>
              <Show when={item.hint}><span class="row-menu-hint">{item.hint}</span></Show>
            </button>
          </>
        )}</For>
      </div>
    </Portal>
  );
};

import { createSignal, createUniqueId, For, onCleanup, onMount, Show, type Component, type JSX } from "solid-js";
import { t } from "../lib/i18n";
import { Icon } from "./icons";

/**
 * The shared filter bar of the list pages: "X of Y" with "Reset filters" on top, then one row with the
 * search, a button per facet and the page's own tools. A facet button opens a popover whose choices
 * apply at once. The bar knows nothing of loading, the address or the server: it gets the facets, the
 * current value and the counts, and reports a new value.
 */

export type FacetOption = {
  value: string;
  /** Already translated. */
  label: string;
  /** A hit here needs attention; its count is marked. */
  urgent?: boolean;
};

export type Facet = {
  /** The key of this facet in a `FilterValue`. */
  key: string;
  /** Already translated. */
  title: string;
  options: ReadonlyArray<FacetOption>;
  /** "or": nothing chosen is everything, chosen values show any of them. "and": an item must have all chosen values. */
  logic: "or" | "and";
  /** Several values at once (the default) or a single one, offered as radio buttons. */
  multiple?: boolean;
  /**
   * The main row (the default) or the second row under "More filters". Reserved for #68: until it
   * exists, "more" facets sit in the main row as well.
   */
  placement?: "main" | "more";
  /** A search field above long option lists. Reserved for #68; not shown yet. */
  searchable?: boolean;
};

/** The chosen values per facet key, in option order. A missing key or an empty list is "nothing chosen". */
export type FilterValue = Readonly<Record<string, ReadonlyArray<string>>>;

/** Per facet key, per option: how many items that option would show with every other facet as it is. */
export type FacetCounts = Readonly<Record<string, Readonly<Record<string, number>>>>;

const chosenOf = (value: FilterValue, key: string): ReadonlyArray<string> => value[key] ?? [];
const activeCount = (facets: ReadonlyArray<Facet>, value: FilterValue) => facets.filter((facet) => chosenOf(value, facet.key).length > 0).length;

/** The value with `option` of `facet` chosen or not, the facet's values kept in option order. */
function toggled(facet: Facet, value: FilterValue, option: string): FilterValue {
  const current = new Set(chosenOf(value, facet.key));
  if (facet.multiple === false) return { ...value, [facet.key]: current.has(option) ? [] : [option] };
  if (current.has(option)) current.delete(option); else current.add(option);
  return { ...value, [facet.key]: facet.options.map((entry) => entry.value).filter((entry) => current.has(entry)) };
}

/** What a facet button says: its title, and while active the chosen values, from three on only how many. */
function chosenText(facet: Facet, value: FilterValue): string {
  const chosen = chosenOf(value, facet.key);
  if (chosen.length === 0) return "";
  if (chosen.length >= 3) return String(chosen.length);
  return chosen.map((entry) => facet.options.find((option) => option.value === entry)?.label ?? entry).join(", ");
}

/** The choices of one facet, as in the popover and in the panel with all filters. */
const Options: Component<{ facet: Facet; value: FilterValue; counts: FacetCounts | null | undefined; onChange: (next: FilterValue) => void; ref?: (element: HTMLElement) => void }> = (props) => {
  const name = createUniqueId();
  const count = (option: string) => props.counts?.[props.facet.key]?.[option];
  // Arrow keys move between the choices; Space ticks one as a native box does.
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const inputs = [...(event.currentTarget as HTMLElement).querySelectorAll<HTMLInputElement>("input")];
    const index = inputs.indexOf(event.target as HTMLInputElement);
    if (index < 0) return;
    event.preventDefault();
    inputs[(index + (event.key === "ArrowDown" ? 1 : -1) + inputs.length) % inputs.length]?.focus();
  };
  return (
    <div ref={props.ref} class="fb-options" role="group" aria-label={props.facet.title} onKeyDown={onKeyDown}>
      <p class="fb-logic">{props.facet.logic === "and" ? t("All of these") : t("Any of these")}</p>
      <For each={props.facet.options}>{(option) => (
        <label class="fb-option" data-empty={count(option.value) === 0 ? "true" : undefined}>
          <input
            type={props.facet.multiple === false ? "radio" : "checkbox"}
            name={name}
            checked={chosenOf(props.value, props.facet.key).includes(option.value)}
            onChange={() => props.onChange(toggled(props.facet, props.value, option.value))}
          />
          <span class="box" aria-hidden="true"><Icon name="check" size={12} /></span>
          <span class="name">{option.label}</span>
          <Show when={count(option.value) !== undefined}>
            <span class="count" classList={{ urgent: !!option.urgent && (count(option.value) ?? 0) > 0 }}>{count(option.value)}</span>
          </Show>
        </label>
      )}</For>
    </div>
  );
};

export const FilterBar: Component<{
  /** The name of the whole bar, e.g. "Filter files". */
  label: string;
  facets: ReadonlyArray<Facet>;
  value: FilterValue;
  /** Null or missing while unknown: no counts are shown and no facet is locked. */
  counts?: FacetCounts | null;
  /** "X of Y files", already worded by the page; announced politely when it changes. */
  summary?: string | null;
  /** Something besides the facets narrows the list (the search), so "Reset filters" shows. */
  dirty?: boolean;
  onChange: (next: FilterValue) => void;
  /** "Reset filters"; without it the bar clears every facet with `onChange({})`. */
  onReset?: () => void;
  /** The search field, first in the row. */
  search?: JSX.Element;
  /** The page's own controls (sort, view), last in the row. */
  tools?: JSX.Element;
}> = (props) => {
  const id = createUniqueId();
  const [open, setOpen] = createSignal<string | null>(null);
  const buttons = new Map<string, HTMLButtonElement>();
  let bar: HTMLDivElement | undefined;

  const active = () => activeCount(props.facets, props.value);
  const resettable = () => active() > 0 || !!props.dirty;
  const reset = () => (props.onReset ? props.onReset() : props.onChange({}));
  const clearFacet = (facet: Facet) => props.onChange({ ...props.value, [facet.key]: [] });
  /** No option would show anything and nothing is chosen: there is nothing to pick. */
  const locked = (facet: Facet) => {
    const counts = props.counts?.[facet.key];
    return !!counts && chosenOf(props.value, facet.key).length === 0 && facet.options.every((option) => (counts[option.value] ?? 0) === 0);
  };
  const close = (refocus = true) => {
    const was = open();
    setOpen(null);
    if (refocus && was) (was === "*" ? allButton : buttons.get(was))?.focus();
  };
  const toggle = (key: string) => (open() === key ? close() : setOpen(key));
  let allButton: HTMLButtonElement | undefined;

  // A click outside the bar closes the popover, as a menu does.
  onMount(() => {
    const onPointer = (event: PointerEvent) => { if (open() && bar && !bar.contains(event.target as Node)) close(false); };
    document.addEventListener("pointerdown", onPointer);
    onCleanup(() => document.removeEventListener("pointerdown", onPointer));
  });
  const onPopoverKey = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    close();
  };

  const Popover: Component<{ title: string; id: string; children: JSX.Element; onReset?: () => void }> = (inner) => (
    <div class="fb-popover" role="dialog" id={inner.id} aria-label={inner.title} onKeyDown={onPopoverKey}>
      <div class="fb-pop-head">
        <b>{inner.title}</b>
        <button type="button" class="fb-close" onClick={() => close()}>{t("Close")}</button>
      </div>
      <div class="fb-pop-body">{inner.children}</div>
      <Show when={inner.onReset}>
        <div class="fb-pop-foot">
          <button type="button" class="fb-link" onClick={() => inner.onReset?.()}>{t("Reset")}</button>
        </div>
      </Show>
    </div>
  );
  const focusFirst = (element: HTMLElement) => queueMicrotask(() => element.querySelector<HTMLInputElement>("input")?.focus());

  return (
    <div class="filterbar" ref={bar} role="search" aria-label={props.label}>
      <div class="fb-head">
        <span class="fb-summary" aria-live="polite">{props.summary ?? ""}</span>
        <Show when={resettable()}>
          <button type="button" class="fb-link" onClick={reset}>{t("Reset filters")}</button>
        </Show>
      </div>
      <div class="fb-row">
        <Show when={props.search}><div class="fb-search">{props.search}</div></Show>
        <div class="fb-facets">
          <span class="fb-facet fb-all-wrap">
            <button
              type="button"
              ref={allButton}
              class="fb-button fb-all"
              data-active={active() > 0 ? "true" : undefined}
              aria-haspopup="dialog"
              aria-expanded={open() === "*"}
              aria-controls={`${id}-all`}
              aria-label={active() > 0 ? t("All filters ({count} active)", { count: active() }) : t("All filters")}
              onClick={() => toggle("*")}
            >
              <Icon name="filter" size={14} />
            </button>
            <Show when={open() === "*"}>
              <Popover title={t("All filters")} id={`${id}-all`}>
                <For each={props.facets}>{(facet, index) => (
                  <section class="fb-all-facet">
                    <h3>{facet.title}</h3>
                    <Options facet={facet} value={props.value} counts={props.counts} onChange={props.onChange} ref={index() === 0 ? focusFirst : undefined} />
                  </section>
                )}</For>
              </Popover>
            </Show>
          </span>
          <For each={props.facets}>{(facet) => {
            const isActive = () => chosenOf(props.value, facet.key).length > 0;
            const popId = `${id}-${facet.key}`;
            return (
              <span class="fb-facet" data-active={isActive() ? "true" : undefined}>
                <span class="fb-split">
                  <button
                    type="button"
                    ref={(element) => buttons.set(facet.key, element)}
                    class="fb-button"
                    data-active={isActive() ? "true" : undefined}
                    disabled={locked(facet)}
                    aria-haspopup="dialog"
                    aria-expanded={open() === facet.key}
                    aria-controls={popId}
                    onClick={() => toggle(facet.key)}
                  >
                    <span class="fb-title">{isActive() ? `${facet.title}: ${chosenText(facet, props.value)}` : facet.title}</span>
                    <Show when={!isActive()}><Icon name="chevron" size={12} /></Show>
                  </button>
                  <Show when={isActive()}>
                    <button type="button" class="fb-clear" aria-label={t("Clear {facet}", { facet: facet.title })} onClick={() => clearFacet(facet)}>
                      <Icon name="x" size={12} />
                    </button>
                  </Show>
                </span>
                <Show when={open() === facet.key}>
                  <Popover title={facet.title} id={popId} onReset={() => clearFacet(facet)}>
                    <Options facet={facet} value={props.value} counts={props.counts} onChange={props.onChange} ref={focusFirst} />
                  </Popover>
                </Show>
              </span>
            );
          }}</For>
        </div>
        <Show when={props.tools}><div class="fb-tools">{props.tools}</div></Show>
      </div>
    </div>
  );
};

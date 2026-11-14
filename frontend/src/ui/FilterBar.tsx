import { createEffect, createSignal, createUniqueId, For, onCleanup, onMount, Show, type Component, type JSX } from "solid-js";
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
   * The main row (the default) or the second row under "More filters", which opens in place and stays
   * open while any facet in it is active.
   */
  placement?: "main" | "more";
  /** A search field above the options, for long lists; it narrows them by label. */
  searchable?: boolean;
  /** One option of the facet stands for a group whose own options show below it. */
  group?: FacetGroup;
};

/**
 * A group within a facet's options, e.g. "Schedules" with each schedule below it. The box of the option
 * `value` chooses the whole group and ticks all of its options; they are chosen one by one under their own
 * `key` of the `FilterValue`, as alternatives to the facet's other options. Choosing the whole group drops
 * the single choices; unticking one of them while the whole group is chosen keeps the others.
 */
export type FacetGroup = {
  /** The facet option that is the box for the whole group. */
  value: string;
  /** The key of the group's own choices in a `FilterValue` and in the counts. */
  key: string;
  options: ReadonlyArray<FacetOption>;
  /** A search field above the group's options; it narrows only them. */
  searchable?: boolean;
};

/** A fixed filter one click away, as the page defines it; `active` while the filter is exactly it. */
export type QuickChoice = { id: string; /** Already translated. */ label: string; active: boolean };

/** The chosen values per facet key, in option order. A missing key or an empty list is "nothing chosen". */
export type FilterValue = Readonly<Record<string, ReadonlyArray<string>>>;

/** Per facet key, per option: how many items that option would show with every other facet as it is. */
export type FacetCounts = Readonly<Record<string, Readonly<Record<string, number>>>>;

const chosenOf = (value: FilterValue, key: string): ReadonlyArray<string> => value[key] ?? [];
/** Anything is chosen in the facet, its group included. */
const facetActive = (facet: Facet, value: FilterValue) => chosenOf(value, facet.key).length > 0 || (!!facet.group && chosenOf(value, facet.group.key).length > 0);
const activeCount = (facets: ReadonlyArray<Facet>, value: FilterValue) => facets.filter((facet) => facetActive(facet, value)).length;

/** The value with `option` of `facet` chosen or not, the facet's values kept in option order. */
function toggled(facet: Facet, value: FilterValue, option: string): FilterValue {
  const current = new Set(chosenOf(value, facet.key));
  if (facet.multiple === false) return { ...value, [facet.key]: current.has(option) ? [] : [option] };
  if (current.has(option)) current.delete(option); else current.add(option);
  return { ...value, [facet.key]: facet.options.map((entry) => entry.value).filter((entry) => current.has(entry)) };
}

/** The value with the whole `group` chosen or not; either way its single choices are dropped. */
function toggledGroup(facet: Facet, group: FacetGroup, value: FilterValue): FilterValue {
  return toggled(facet, { ...value, [group.key]: [] }, group.value);
}

/** The value with `option` of `group` chosen or not; unticked while the whole group is chosen, the others stay. */
function toggledInGroup(facet: Facet, group: FacetGroup, value: FilterValue, option: string): FilterValue {
  const all = group.options.map((entry) => entry.value);
  if (chosenOf(value, facet.key).includes(group.value)) {
    return { ...toggled(facet, value, group.value), [group.key]: all.filter((entry) => entry !== option) };
  }
  const current = new Set(chosenOf(value, group.key));
  if (current.has(option)) current.delete(option); else current.add(option);
  return { ...value, [group.key]: [...all.filter((entry) => current.has(entry)), ...[...current].filter((entry) => !all.includes(entry))] };
}

/** What a facet button says: its title, and while active the chosen values, from three on only how many. */
function chosenText(facet: Facet, value: FilterValue): string {
  const labelOf = (options: ReadonlyArray<FacetOption>) => (entry: string) => options.find((option) => option.value === entry)?.label ?? entry;
  const chosen = [
    ...chosenOf(value, facet.key).map(labelOf(facet.options)),
    ...(facet.group ? chosenOf(value, facet.group.key).map(labelOf(facet.group.options)) : []),
  ];
  if (chosen.length === 0) return "";
  if (chosen.length >= 3) return String(chosen.length);
  return chosen.join(", ");
}

/** A search field's text and the options whose label contains it. */
function narrowing(options: () => ReadonlyArray<FacetOption>) {
  const [find, setFind] = createSignal("");
  const shown = () => {
    const wanted = find().trim().toLocaleLowerCase();
    return wanted ? options().filter((option) => option.label.toLocaleLowerCase().includes(wanted)) : options();
  };
  return { find, setFind, shown };
}

/** One box with its label and count. */
const OptionBox: Component<{ name: string; type: "checkbox" | "radio"; label: string; count: number | undefined; urgent?: boolean | undefined; checked: boolean; mixed?: boolean; onChange: () => void }> = (props) => {
  let input: HTMLInputElement | undefined;
  // A group partly chosen shows a dash; the state exists only as a property of the element.
  createEffect(() => { if (input) input.indeterminate = !!props.mixed && !props.checked; });
  return (
    <label class="fb-option" data-empty={props.count === 0 ? "true" : undefined}>
      <input ref={input} type={props.type} name={props.name} checked={props.checked} onChange={() => props.onChange()} />
      <span class="box" aria-hidden="true"><Icon name="check" size={12} /></span>
      <span class="name">{props.label}</span>
      <Show when={props.count !== undefined}>
        <span class="count" classList={{ urgent: !!props.urgent && (props.count ?? 0) > 0 }}>{props.count}</span>
      </Show>
    </label>
  );
};

/** The box for a whole group and below it the group's own options, with their search. */
const GroupOptions: Component<{ facet: Facet; group: FacetGroup; label: string; value: FilterValue; counts: FacetCounts | null | undefined; onChange: (next: FilterValue) => void }> = (props) => {
  const name = createUniqueId();
  const { find, setFind, shown } = narrowing(() => props.group.options);
  const whole = () => chosenOf(props.value, props.facet.key).includes(props.group.value);
  const single = () => chosenOf(props.value, props.group.key);
  return (
    <div class="fb-group" role="group" aria-label={props.label}>
      <OptionBox
        name={name}
        type="checkbox"
        label={props.label}
        count={props.counts?.[props.facet.key]?.[props.group.value]}
        checked={whole()}
        mixed={single().length > 0}
        onChange={() => props.onChange(toggledGroup(props.facet, props.group, props.value))}
      />
      <div class="fb-group-options">
        <Show when={props.group.searchable}>
          <input
            class="input fb-find"
            type="search"
            aria-label={t("Search {facet}", { facet: props.label })}
            placeholder={t("Search")}
            value={find()}
            onInput={(event) => setFind(event.currentTarget.value)}
          />
          <Show when={shown().length === 0}><p class="fb-logic">{t("Nothing matches")}</p></Show>
        </Show>
        {/* Kept by value, as the facet's own options are. */}
        <For each={shown().map((option) => option.value)}>{(value) => {
          const option = () => props.group.options.find((entry) => entry.value === value) ?? { value, label: value };
          return (
            <OptionBox
              name={name}
              type="checkbox"
              label={option().label}
              count={props.counts?.[props.group.key]?.[value]}
              urgent={option().urgent}
              checked={whole() || single().includes(value)}
              onChange={() => props.onChange(toggledInGroup(props.facet, props.group, props.value, value))}
            />
          );
        }}</For>
      </div>
    </div>
  );
};

/** The choices of one facet, as in the popover and in the panel with all filters. */
const Options: Component<{ facet: Facet; value: FilterValue; counts: FacetCounts | null | undefined; onChange: (next: FilterValue) => void; ref?: (element: HTMLElement) => void }> = (props) => {
  const name = createUniqueId();
  const count = (option: string) => props.counts?.[props.facet.key]?.[option];
  const { find, setFind, shown } = narrowing(() => props.facet.options);
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
      <Show when={props.facet.searchable}>
        <input
          class="input fb-find"
          type="search"
          aria-label={t("Search {facet}", { facet: props.facet.title })}
          placeholder={t("Search")}
          value={find()}
          onInput={(event) => setFind(event.currentTarget.value)}
        />
      </Show>
      <p class="fb-logic">{props.facet.logic === "and" ? t("All of these") : t("Any of these")}</p>
      <Show when={shown().length === 0}><p class="fb-logic">{t("Nothing matches")}</p></Show>
      {/* Kept by value, so options handed in anew keep their elements and the focus. */}
      <For each={shown().map((option) => option.value)}>{(value) => {
        const option = () => props.facet.options.find((entry) => entry.value === value) ?? { value, label: value };
        const group = () => (props.facet.group?.value === value ? props.facet.group : undefined);
        return (
          <Show when={group()} fallback={(
            <OptionBox
              name={name}
              type={props.facet.multiple === false ? "radio" : "checkbox"}
              label={option().label}
              count={count(value)}
              urgent={option().urgent}
              checked={chosenOf(props.value, props.facet.key).includes(value)}
              onChange={() => props.onChange(toggled(props.facet, props.value, value))}
            />
          )}>
            {(inner) => <GroupOptions facet={props.facet} group={inner()} label={option().label} value={props.value} counts={props.counts} onChange={props.onChange} />}
          </Show>
        );
      }}</For>
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
  /** Fixed filters one click away, shown next to "X of Y". */
  quick?: ReadonlyArray<QuickChoice>;
  /** A quick filter was clicked; the page sets the filter it stands for. */
  onQuick?: (id: string) => void;
}> = (props) => {
  const id = createUniqueId();
  const [open, setOpen] = createSignal<string | null>(null);
  const [moreOpen, setMoreOpen] = createSignal(false);
  const mainFacets = () => props.facets.filter((facet) => facet.placement !== "more");
  const moreFacets = () => props.facets.filter((facet) => facet.placement === "more");
  /** An active facet in the second row keeps it open, so no active filter is ever hidden. */
  const moreActive = () => activeCount(moreFacets(), props.value) > 0;
  const moreShown = () => moreFacets().length > 0 && (moreOpen() || moreActive());
  const buttons = new Map<string, HTMLButtonElement>();
  let bar: HTMLDivElement | undefined;

  const active = () => activeCount(props.facets, props.value);
  const resettable = () => active() > 0 || !!props.dirty;
  const reset = () => (props.onReset ? props.onReset() : props.onChange({}));
  const clearFacet = (facet: Facet) => props.onChange({ ...props.value, [facet.key]: [], ...(facet.group ? { [facet.group.key]: [] } : {}) });
  /** No option would show anything and nothing is chosen: there is nothing to pick. */
  const locked = (facet: Facet) => {
    const counts = props.counts?.[facet.key];
    return !!counts && !facetActive(facet, props.value) && facet.options.every((option) => (counts[option.value] ?? 0) === 0);
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

  /**
   * The facets are kept by key, so a page that builds its facets anew (options from the counts) does not
   * rebuild the buttons and an open popover under the pointer.
   */
  const keysOf = (facets: ReadonlyArray<Facet>) => facets.map((facet) => facet.key);
  const facetOf = (key: string): Facet => props.facets.find((facet) => facet.key === key) ?? { key, title: key, options: [], logic: "or" };

  /** One facet button with its ✕ and popover, in either row. */
  const facetItem = (key: string) => {
    const facet = () => facetOf(key);
    const isActive = () => facetActive(facet(), props.value);
    const popId = `${id}-${key}`;
    return (
      <span class="fb-facet" data-active={isActive() ? "true" : undefined}>
        <span class="fb-split">
          <button
            type="button"
            ref={(element) => buttons.set(key, element)}
            class="fb-button"
            data-active={isActive() ? "true" : undefined}
            disabled={locked(facet())}
            aria-haspopup="dialog"
            aria-expanded={open() === key}
            aria-controls={popId}
            onClick={() => toggle(key)}
          >
            <span class="fb-title">{isActive() ? `${facet().title}: ${chosenText(facet(), props.value)}` : facet().title}</span>
            <Show when={!isActive()}><Icon name="chevron" size={12} /></Show>
          </button>
          <Show when={isActive()}>
            <button type="button" class="fb-clear" aria-label={t("Clear {facet}", { facet: facet().title })} onClick={() => clearFacet(facet())}>
              <Icon name="x" size={12} />
            </button>
          </Show>
        </span>
        <Show when={open() === key}>
          <Popover title={facet().title} id={popId} onReset={() => clearFacet(facet())}>
            <Options facet={facet()} value={props.value} counts={props.counts} onChange={props.onChange} ref={focusFirst} />
          </Popover>
        </Show>
      </span>
    );
  };

  return (
    <div class="filterbar" ref={bar} role="search" aria-label={props.label}>
      <div class="fb-head">
        <span class="fb-summary" aria-live="polite">{props.summary ?? ""}</span>
        <Show when={(props.quick?.length ?? 0) > 0}>
          <div class="fb-quick" role="group" aria-label={t("Quick filters")}>
            <For each={props.quick}>{(choice) => (
              <button type="button" class="fb-quick-button" aria-pressed={choice.active} onClick={() => props.onQuick?.(choice.id)}>{choice.label}</button>
            )}</For>
          </div>
        </Show>
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
                <For each={keysOf(props.facets)}>{(key, index) => (
                  <section class="fb-all-facet">
                    <h3>{facetOf(key).title}</h3>
                    <Options facet={facetOf(key)} value={props.value} counts={props.counts} onChange={props.onChange} ref={index() === 0 ? focusFirst : undefined} />
                  </section>
                )}</For>
              </Popover>
            </Show>
          </span>
          <For each={keysOf(mainFacets())}>{facetItem}</For>
          <Show when={moreFacets().length > 0}>
            <button
              type="button"
              class="fb-link fb-more-toggle"
              aria-expanded={moreShown()}
              aria-controls={`${id}-more`}
              disabled={moreActive()}
              title={moreActive() ? t("Stays open while a filter in it is active") : undefined}
              onClick={() => setMoreOpen(!moreOpen())}
            >
              {t("More filters")}
            </button>
          </Show>
        </div>
        <Show when={props.tools}><div class="fb-tools">{props.tools}</div></Show>
      </div>
      <Show when={moreShown()}>
        <div class="fb-row fb-more" id={`${id}-more`} role="group" aria-label={t("More filters")}>
          <div class="fb-facets">
            <For each={keysOf(moreFacets())}>{facetItem}</For>
          </div>
        </div>
      </Show>
    </div>
  );
};

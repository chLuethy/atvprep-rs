import { For, Show, type Component } from "solid-js";
import { Button } from "../../ui";
import { t } from "../../lib/i18n";
import { actionName, LIST_ACTIONS, LIST_STATUSES, SORT_KEYS, type ListFilter, type SortKey } from "./model";

const toggled = <T,>(list: T[], item: T): T[] => (list.includes(item) ? list.filter((known) => known !== item) : [...list, item]);

/** Sort choice and filter chips above the schedule list. */
export const ListToolbar: Component<{
  sort: SortKey; filter: ListFilter; filtered: boolean;
  onSort: (key: SortKey) => void; onFilter: (filter: ListFilter) => void; onReset: () => void;
}> = (props) => (
  <div class="sched-toolbar">
    <label class="sched-sort">
      <span class="muted">{t("Sort by")}</span>
      <select class="select" value={props.sort} onChange={(event) => props.onSort(event.currentTarget.value as SortKey)}>
        <For each={SORT_KEYS}>{(choice) => <option value={choice.key}>{t(choice.label)}</option>}</For>
      </select>
    </label>
    <div class="chips" role="group" aria-label={t("Filter schedules by action")}>
      <For each={LIST_ACTIONS}>{(action) => (
        <button type="button" class="filter-chip" aria-pressed={props.filter.actions.includes(action)} onClick={() => props.onFilter({ ...props.filter, actions: toggled(props.filter.actions, action) })}>{actionName(action)}</button>
      )}</For>
    </div>
    <div class="chips" role="group" aria-label={t("Filter schedules by status")}>
      <For each={LIST_STATUSES}>{(choice) => (
        <button type="button" class="filter-chip" aria-pressed={props.filter.statuses.includes(choice.status)} onClick={() => props.onFilter({ ...props.filter, statuses: toggled(props.filter.statuses, choice.status) })}>{t(choice.label)}</button>
      )}</For>
    </div>
    <Show when={props.filtered}><Button variant="quiet" onClick={props.onReset}>{t("Reset filters")}</Button></Show>
  </div>
);

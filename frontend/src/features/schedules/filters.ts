import { list, type FacetFilterSpec } from "../../lib/facetFilter";
import type { Action } from "./loaders";
import { LIST_ACTIONS, LIST_STATUSES, SORT_KEYS, type ListFilter, type SortKey, type StatusFilter } from "./model";

/**
 * The schedule list's view and its address. Actions and statuses: nothing chosen shows every schedule, chosen
 * values show the schedules with any of them (OR); the address says them as `action` and `status`, the order as
 * `sort` (next run unless it says otherwise). The list is filtered and sorted here in the browser: there are few
 * schedules, so the server is not asked.
 */
export type ScheduleView = ListFilter & { sort: SortKey };

export const EMPTY_SCHEDULE_VIEW: ScheduleView = { actions: [], statuses: [], sort: "next" };

const ACTION_ORDER: ReadonlyArray<Action> = LIST_ACTIONS;
const STATUS_ORDER: ReadonlyArray<StatusFilter> = LIST_STATUSES.map((choice) => choice.status);
const SORT_ORDER: ReadonlyArray<SortKey> = SORT_KEYS.map((choice) => choice.key);

/** The known values among `values`, once each, in the order of `order`: the order's own type, no cast. */
const known = <T extends string>(order: ReadonlyArray<T>, values: Iterable<string>): T[] => {
  const wanted = new Set(values);
  return order.filter((token) => wanted.has(token));
};
const isSortKey = (value: string | null): value is SortKey => SORT_ORDER.some((key) => key === value);

/** The view an address asks for; unknown values are dropped. */
export function parseScheduleView(params: URLSearchParams): ScheduleView {
  const sort = params.get("sort");
  return {
    actions: known(ACTION_ORDER, list(params.get("action"))),
    statuses: known(STATUS_ORDER, list(params.get("status"))),
    sort: isSortKey(sort) ? sort : "next",
  };
}

/** The canonical query of a view, without "?": `action`, `status`, then `sort`; "" for the default view. */
export function scheduleViewQuery(view: ScheduleView): string {
  const parts: string[] = [];
  const actions = known(ACTION_ORDER, view.actions);
  const statuses = known(STATUS_ORDER, view.statuses);
  if (actions.length > 0) parts.push(`action=${actions.join(",")}`);
  if (statuses.length > 0) parts.push(`status=${statuses.join(",")}`);
  if (view.sort !== "next") parts.push(`sort=${view.sort}`);
  return parts.join("&");
}

export const sameScheduleView = (a: ScheduleView, b: ScheduleView): boolean => scheduleViewQuery(a) === scheduleViewQuery(b);

/** Anything at all narrows the list; the order does not. */
export const isScheduleFiltered = (view: ScheduleView): boolean => view.actions.length + view.statuses.length > 0;

/** How the schedule view relates to its filter chips: the facets as a value and back; the order stays. */
export const SCHEDULE_FILTER_SPEC: FacetFilterSpec<ScheduleView> = {
  empty: EMPTY_SCHEDULE_VIEW,
  same: sameScheduleView,
  isFiltered: isScheduleFiltered,
  keep: (view) => ({ sort: view.sort }),
  toValue: (view) => ({ action: view.actions, status: view.statuses }),
  fromValue: (value, current) => ({
    ...current,
    actions: known(ACTION_ORDER, value.action ?? []),
    statuses: known(STATUS_ORDER, value.status ?? []),
  }),
};

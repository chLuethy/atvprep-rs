import { createResource } from "solid-js";
import { locale, N, t } from "../../lib/i18n";
import { MANY_OPTIONS, type Facet, type FacetCounts, type FilterValue } from "../../ui/FilterBar";
import { loadRoots } from "../scan/loaders";
import type { JobFilter } from "./filters";
import type { Api } from "./loaders";
import { customPeriod, dayOf, dayStart, PERIOD_PRESETS, type JobPeriod } from "./period";

/** The option of the period facet that stands for fixed days. */
const CUSTOM = "custom";
const CUSTOM_LABEL = N("Custom period");

/** A day of a custom period as the language writes it. */
const dayText = (day: string) => dayStart(day)?.toLocaleDateString(locale(), { dateStyle: "medium" }) ?? day;

/**
 * The period and library facets of the activity filter (#71), both under "More filters". The period is one
 * choice: a preset or a custom period, whose days are set in two fields under the options; choosing the custom
 * period starts with today. The libraries come from the configured roots; a chosen one the server no longer
 * lists stays as its id, so a choice never vanishes.
 */
export function periodLibraryFacets(props: { api: Api; filter: () => JobFilter; counts: () => FacetCounts | null; onChange: (next: JobFilter) => void }) {
  // The names are read once the server counts per library (or one is chosen), not before the list.
  const wanted = () => !!props.counts()?.root || (props.filter().root?.length ?? 0) > 0;
  const [roots] = createResource(wanted, async () => {
    const answer = await loadRoots(props.api);
    return answer.kind === "ok" ? answer.items : [];
  }, { initialValue: [] });

  const custom = () => {
    const period = props.filter().period;
    return period && !("preset" in period) ? period : null;
  };
  const withPeriod = (filter: JobFilter, period: JobPeriod | undefined): JobFilter => {
    const { period: _old, ...rest } = filter;
    return period ? { ...rest, period } : rest;
  };
  const setDay = (which: "from" | "to", day: string) => {
    const known = custom();
    const other = which === "from" ? known?.to ?? day : known?.from ?? day;
    const period = which === "from" ? customPeriod(day, other) : customPeriod(other, day);
    if (period) props.onChange(withPeriod(props.filter(), period));
  };
  /** Built once per open popover; the fields follow the filter themselves. */
  const Days = () => (
    <div class="fb-days" role="group" aria-label={t(CUSTOM_LABEL)}>
      <label>
        <span>{t("From")}</span>
        <input class="input" type="date" value={custom()?.from ?? ""} onChange={(event) => setDay("from", event.currentTarget.value)} />
      </label>
      <label>
        <span>{t("To")}</span>
        <input class="input" type="date" value={custom()?.to ?? ""} onChange={(event) => setDay("to", event.currentTarget.value)} />
      </label>
    </div>
  );

  const facets = (): Facet[] => {
    const days = custom();
    const known = roots().map((root) => ({ value: root.id, label: root.name }));
    const seen = new Set(known.map((option) => option.value));
    const chosen = (props.filter().root ?? []).filter((id) => !seen.has(id)).map((id) => ({ value: id, label: id }));
    return [
      {
        key: "period", title: t("Period"), logic: "or", multiple: false, placement: "more", extra: Days,
        options: [
          ...PERIOD_PRESETS.map((entry) => ({ value: entry.token, label: t(entry.label) })),
          { value: CUSTOM, label: days ? `${dayText(days.from)} – ${dayText(days.to)}` : t(CUSTOM_LABEL) },
        ],
      },
      { key: "root", title: t("Library"), logic: "or", placement: "more", searchable: known.length > MANY_OPTIONS, options: [...known, ...chosen] },
    ];
  };
  const value = (): FilterValue => {
    const period = props.filter().period;
    return { period: !period ? [] : "preset" in period ? [period.preset] : [CUSTOM], root: props.filter().root ?? [] };
  };
  /** `base` with the period and libraries the bar reports. */
  const apply = (next: FilterValue, base: JobFilter): JobFilter => {
    const picked = next.period?.[0];
    const preset = PERIOD_PRESETS.find((entry) => entry.token === picked);
    const today = dayOf(new Date());
    const period: JobPeriod | undefined = preset ? { preset: preset.token } : picked === CUSTOM ? custom() ?? { from: today, to: today } : undefined;
    const { root: _old, ...rest } = withPeriod(base, period);
    const root = [...(next.root ?? [])].sort();
    return root.length > 0 ? { ...rest, root } : rest;
  };
  return { facets, value, apply };
}

import { describe, expect, it } from "vitest";
import { EMPTY_JOB_FILTER, isJobFiltered, jobFilterQuery, jobListParams, parseJobFilter, sameJobFilter } from "../src/features/activity/filters";
import { periodBounds } from "../src/features/activity/period";

const parse = (search: string) => parseJobFilter(new URLSearchParams(search));
/** A local wall-clock time, as the browser's zone reads it. */
const local = (month: number, day: number, hour = 0, minute = 0) => new Date(2026, month - 1, day, hour, minute);

describe("activity period: the bounds", () => {
  const now = local(11, 14, 9, 45);

  it("starts today at midnight in the browser's zone, also just after a change of day", () => {
    expect(periodBounds({ preset: "today" }, now)).toEqual({ from: local(11, 14).toISOString() });
    expect(periodBounds({ preset: "today" }, local(11, 14, 0, 5))).toEqual({ from: local(11, 14).toISOString() });
    expect(periodBounds({ preset: "today" }, local(11, 13, 23, 55))).toEqual({ from: local(11, 13).toISOString() });
  });

  it("goes back 24 hours, 7 and 30 days from now, by the wall clock across a change of daylight saving", () => {
    expect(periodBounds({ preset: "24h" }, now)).toEqual({ from: new Date(now.getTime() - 24 * 3600 * 1000).toISOString() });
    expect(periodBounds({ preset: "7d" }, now)).toEqual({ from: local(11, 7, 9, 45).toISOString() });
    expect(periodBounds({ preset: "30d" }, now)).toEqual({ from: local(10, 15, 9, 45).toISOString() });
  });

  it("takes a custom period as whole days, the last one included", () => {
    expect(periodBounds({ from: "2026-11-01", to: "2026-11-05" }, now)).toEqual({ from: local(11, 1).toISOString(), to: local(11, 6).toISOString() });
    expect(periodBounds({ from: "2026-12-31", to: "2026-12-31" }, now)).toEqual({ from: new Date(2026, 11, 31).toISOString(), to: new Date(2027, 0, 1).toISOString() });
  });
});

describe("activity period and library: the address", () => {
  it("keeps a preset as its name and a custom period as fixed dates", () => {
    expect(parse("range=24h").period).toEqual({ preset: "24h" });
    expect(jobFilterQuery({ ...EMPTY_JOB_FILTER, period: { preset: "7d" } })).toBe("range=7d");
    expect(parse("from=2026-11-01&to=2026-11-05").period).toEqual({ from: "2026-11-01", to: "2026-11-05" });
    expect(jobFilterQuery({ ...EMPTY_JOB_FILTER, period: { from: "2026-11-01", to: "2026-11-05" } })).toBe("from=2026-11-01&to=2026-11-05");
  });

  it("tidies a custom period: one day given is that day, swapped days are put in order, nonsense is dropped", () => {
    expect(parse("from=2026-11-03").period).toEqual({ from: "2026-11-03", to: "2026-11-03" });
    expect(parse("from=2026-11-05&to=2026-11-01").period).toEqual({ from: "2026-11-01", to: "2026-11-05" });
    expect(parse("from=2026-02-30&to=soon").period).toBeUndefined();
    expect(parse("range=forever").period).toBeUndefined();
    expect(parse("")).toEqual(EMPTY_JOB_FILTER);
  });

  it("reads libraries as root_id, once each and sorted, after the search and before the order", () => {
    expect(parse("root_id=r_b,r_a,r_b").root).toEqual(["r_a", "r_b"]);
    expect(jobFilterQuery({ ...EMPTY_JOB_FILTER, q: "Heat", root: ["r_b", "r_a"], period: { preset: "today" }, sort: "oldest" }))
      .toBe("q=Heat&range=today&root_id=r_a,r_b&sort=oldest");
  });

  it("counts a period or a library as filtered", () => {
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, period: { preset: "today" } })).toBe(true);
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, root: ["r_a"] })).toBe(true);
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, root: [] })).toBe(false);
    expect(sameJobFilter({ ...EMPTY_JOB_FILTER, root: [] }, EMPTY_JOB_FILTER)).toBe(true);
  });
});

describe("activity period and library: the server query", () => {
  it("sends the period as absolute instants worked out at the time of asking, and the libraries as root_id", () => {
    const now = local(11, 14, 0, 30);
    expect(jobListParams({ ...EMPTY_JOB_FILTER, period: { preset: "today" }, root: ["r_a"] }, now))
      .toBe(`order=desc&from=${encodeURIComponent(local(11, 14).toISOString())}&root_id=r_a`);
    expect(jobListParams({ ...EMPTY_JOB_FILTER, period: { from: "2026-11-01", to: "2026-11-01" } }, now))
      .toBe(`order=desc&from=${encodeURIComponent(local(11, 1).toISOString())}&to=${encodeURIComponent(local(11, 2).toISOString())}`);
  });
});

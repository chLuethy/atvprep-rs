import { describe, expect, it } from "vitest";
import { EMPTY_JOB_FILTER, isJobFiltered, JOB_ACTIONS, JOB_STATUSES, JOB_TRIGGERS, jobFilterQuery, jobListParams, parseJobFilter, revealActive } from "../src/features/activity/filters";

const parse = (search: string) => parseJobFilter(new URLSearchParams(search));

describe("activity filter: the entries", () => {
  it("offers all seven job statuses and the three actions", () => {
    expect(JOB_STATUSES.map((entry) => entry.token)).toEqual(["queued", "running", "cancel_requested", "succeeded", "partial_success", "failed", "cancelled"]);
    expect(JOB_ACTIONS.map((entry) => entry.token)).toEqual(["verify", "prepare", "inspect"]);
  });
});

describe("activity filter: the address", () => {
  it("reads statuses, actions, the search and the order", () => {
    expect(parse("status=failed,queued&action=inspect&q=Heat&sort=oldest")).toEqual({ q: "Heat", status: ["queued", "failed"], action: ["inspect"], sort: "oldest" });
    expect(parse("")).toEqual(EMPTY_JOB_FILTER);
  });

  it("keeps only known values, once each, in the canonical order", () => {
    expect(parse("status=cancelled,bogus,queued,queued&action=fix,prepare&sort=sideways")).toEqual({ q: "", status: ["queued", "cancelled"], action: ["prepare"], sort: "newest" });
  });

  it("writes statuses, actions, the search, then the order; the newest-first default is left out", () => {
    expect(jobFilterQuery({ q: "Pretty Woman & Co", status: ["failed", "partial_success"], action: ["verify"], sort: "oldest" }))
      .toBe("status=partial_success,failed&action=verify&q=Pretty%20Woman%20%26%20Co&sort=oldest");
    expect(jobFilterQuery(EMPTY_JOB_FILTER)).toBe("");
  });

  it("reads back what it writes", () => {
    const filter = { q: "a,b c", status: ["running", "cancel_requested"], action: ["inspect", "prepare"].sort(), sort: "oldest" as const };
    const written = parse(jobFilterQuery(filter));
    expect(written).toEqual({ ...filter, action: ["prepare", "inspect"] });
  });

  it("counts only what narrows the list as filtered, not the order", () => {
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, sort: "oldest" })).toBe(false);
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, q: "x" })).toBe(true);
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, action: ["verify"] })).toBe(true);
  });
});

describe("activity filter: the server query", () => {
  it("asks for every job status when nothing is chosen, newest first", () => {
    expect(jobListParams(EMPTY_JOB_FILTER)).toBe("order=desc");
  });

  it("sends the lists with plain commas and the order", () => {
    expect(jobListParams({ q: "Heat & Co", status: ["queued", "failed"], action: ["prepare"], sort: "oldest" }))
      .toBe("order=asc&status=queued,failed&action=prepare&q=Heat%20%26%20Co");
  });
});

describe("activity filter: the Auslöser", () => {
  it("offers by hand, retry and the schedules as a group", () => {
    expect(JOB_TRIGGERS.map((entry) => entry.token)).toEqual(["manual", "retry", "schedule"]);
  });

  it("reads the Auslöser and single schedules, dropping what is no schedule id", () => {
    expect(parse("trigger=schedule,bogus,manual&schedule=sc_b,sc_a-1,sc_b,j_x,a%20b")).toEqual({ ...EMPTY_JOB_FILTER, trigger: ["manual", "schedule"], schedule: ["sc_a-1", "sc_b"] });
    expect(parse("trigger=&schedule=")).toEqual(EMPTY_JOB_FILTER);
  });

  it("writes them after the action and before the search, for the address and the server alike", () => {
    const filter = { ...EMPTY_JOB_FILTER, action: ["verify"], q: "Heat", trigger: ["schedule", "manual"], schedule: ["sc_x"] };
    expect(jobFilterQuery(filter)).toBe("action=verify&trigger=manual,schedule&schedule=sc_x&q=Heat");
    expect(jobListParams(filter)).toBe("order=desc&action=verify&trigger=manual,schedule&schedule=sc_x&q=Heat");
    expect(parse(jobFilterQuery(filter))).toEqual({ ...filter, trigger: ["manual", "schedule"] });
  });

  it("counts a single schedule as a filter", () => {
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, schedule: ["sc_x"] })).toBe(true);
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, trigger: ["retry"] })).toBe(true);
    expect(isJobFiltered({ ...EMPTY_JOB_FILTER, trigger: [], schedule: [] })).toBe(false);
  });
});

describe("activity filter: showing hidden active jobs", () => {
  const counts = (status: Record<string, number>, hidden: number) => ({ facets: { status, action: {} }, matched: 1, total: 9, hiddenActive: hidden });

  it("adds the active statuses when only the status choice hides them", () => {
    const filter = { ...EMPTY_JOB_FILTER, status: ["failed"], action: ["verify"], sort: "oldest" as const };
    // Two waiting and one running verification would show without the status choice.
    expect(revealActive(filter, counts({ queued: 2, running: 1, cancel_requested: 0, failed: 1 }, 3)))
      .toEqual({ ...filter, status: ["queued", "running", "cancel_requested", "failed"] });
  });

  it("clears the filters but keeps the order when the action or the search hides them too", () => {
    const filter = { q: "Heat", status: ["failed"], action: [], sort: "oldest" as const };
    expect(revealActive(filter, counts({ queued: 1, running: 0, cancel_requested: 0, failed: 1 }, 3)))
      .toEqual({ ...EMPTY_JOB_FILTER, sort: "oldest" });
  });
});

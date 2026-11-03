import { describe, expect, it } from "vitest";
import {
  actionLabel,
  canCancel,
  canRetry,
  formatElapsed,
  isActive,
  itemTone,
  jobProgress,
  jobTitle,
  jobPercent,
  jobTone,
  partitionJobs,
  runningFor,
} from "../src/features/activity/model";

const job = (over: Partial<Parameters<typeof jobTitle>[0]> = {}) => ({
  id: "j_1", action: "fix", status: "succeeded", trigger: "manual", origin_job_id: null,
  requested_at: null, started_at: null, finished_at: null,
  total_count: 1, succeeded_count: 1, failed_count: 0, cancelled_count: 0, subject: "Film/Heat (1995)/Heat.mp4", ...over,
});

describe("jobTone", () => {
  it("words every job status and pairs it with a symbol", () => {
    expect(jobTone("queued")).toMatchObject({ kind: "neutral", label: "Queued" });
    expect(jobTone("running")).toMatchObject({ kind: "neutral", label: "Running", icon: "activity" });
    expect(jobTone("cancel_requested")).toMatchObject({ kind: "warn", label: "Cancelling" });
    expect(jobTone("succeeded")).toMatchObject({ kind: "ok", label: "Succeeded" });
    expect(jobTone("partial_success")).toMatchObject({ kind: "warn", label: "Partly failed" });
    expect(jobTone("failed")).toMatchObject({ kind: "bad", label: "Failed" });
    expect(jobTone("cancelled")).toMatchObject({ kind: "neutral", label: "Cancelled" });
    expect(jobTone("something_new")).toMatchObject({ kind: "neutral", label: "Something new" });
  });
});

describe("itemTone", () => {
  it("explains the item states a user would otherwise have to decode", () => {
    expect(itemTone("stale_input")).toMatchObject({ kind: "warn", label: "File changed" });
    expect(itemTone("interrupted")).toMatchObject({ kind: "warn", label: "Interrupted" });
    expect(itemTone("failed")).toMatchObject({ kind: "bad", label: "Failed" });
    expect(itemTone("succeeded")).toMatchObject({ kind: "ok", label: "Succeeded" });
    expect(itemTone("queued")).toMatchObject({ kind: "neutral", label: "Queued" });
  });
});

describe("actions the server accepts", () => {
  it("offers Cancel only while a job can still be stopped", () => {
    expect(canCancel(job({ status: "queued" }))).toBe(true);
    expect(canCancel(job({ status: "running" }))).toBe(true);
    for (const status of ["cancel_requested", "succeeded", "failed", "partial_success", "cancelled"]) {
      expect(canCancel(job({ status }))).toBe(false);
    }
  });

  it("offers Retry only for finished jobs that have something to retry", () => {
    expect(canRetry(job({ status: "failed" }))).toBe(true);
    expect(canRetry(job({ status: "partial_success" }))).toBe(true);
    for (const status of ["queued", "running", "cancel_requested", "succeeded", "cancelled"]) {
      expect(canRetry(job({ status }))).toBe(false);
    }
  });

  it("treats queued, running and cancelling jobs as active", () => {
    expect(["queued", "running", "cancel_requested"].every((status) => isActive(job({ status })))).toBe(true);
    expect(["succeeded", "failed", "partial_success", "cancelled"].some((status) => isActive(job({ status })))).toBe(false);
  });
});

describe("labels and progress", () => {
  it("names the action in words", () => {
    expect(actionLabel("prepare")).toBe("Prepare");
    expect(actionLabel("fix")).toBe("Fix");
    expect(actionLabel("verify")).toBe("Verify");
    expect(actionLabel("inspect")).toBe("Inspect");
    expect(actionLabel("convert")).toBe("Convert");
    expect(actionLabel("odd_one")).toBe("Odd one");
  });

  it("titles a job by its file, and by the count when it covers several", () => {
    expect(jobTitle(job())).toBe("Heat");
    expect(jobTitle(job({ total_count: 3 }))).toBe("Heat and 2 more");
    expect(jobTitle(job({ subject: null }))).toBe("Fix job");
    expect(jobTitle(job({ subject: null, total_count: 4 }))).toBe("Fix job for 4 files");
  });

  it("summarises progress in plain words", () => {
    expect(jobProgress(job({ total_count: 2, succeeded_count: 1, failed_count: 1 }))).toBe("1 of 2 succeeded, 1 failed");
    expect(jobProgress(job({ total_count: 3, succeeded_count: 3 }))).toBe("3 of 3 succeeded");
    expect(jobProgress(job({ total_count: 2, succeeded_count: 0, cancelled_count: 2 }))).toBe("0 of 2 succeeded, 2 cancelled");
    expect(jobProgress(job({ total_count: 1, succeeded_count: 0 }))).toBe("0 of 1 succeeded");
  });
});

describe("formatElapsed", () => {
  it("reads a duration between two timestamps and says nothing without both", () => {
    expect(formatElapsed("2026-07-20T10:00:00Z", "2026-07-20T10:00:42Z")).toBe("42 s");
    expect(formatElapsed("2026-07-20T10:00:00Z", "2026-07-20T10:04:12Z")).toBe("4 min 12 s");
    expect(formatElapsed("2026-07-20T10:00:00Z", "2026-07-20T11:05:00Z")).toBe("1 h 5 min");
    expect(formatElapsed(null, "2026-07-20T10:00:42Z")).toBe("");
    expect(formatElapsed("2026-07-20T10:00:00Z", null)).toBe("");
    expect(formatElapsed("2026-07-20T10:00:10Z", "2026-07-20T10:00:00Z")).toBe("");
  });
});

describe("partitionJobs", () => {
  it("puts active jobs first and keeps each group in the order it arrived", () => {
    const rows = [
      job({ id: "j_5", status: "succeeded" }),
      job({ id: "j_4", status: "running" }),
      job({ id: "j_3", status: "failed" }),
      job({ id: "j_2", status: "queued" }),
    ];

    const { active, history } = partitionJobs(rows);

    expect(active.map((row) => row.id)).toEqual(["j_4", "j_2"]);
    expect(history.map((row) => row.id)).toEqual(["j_5", "j_3"]);
  });
});

describe("jobPercent", () => {
  it("is the server's whole percent for a running job and nothing otherwise", () => {
    expect(jobPercent(job({ status: "running", progress: 42 }))).toBe(42);
    expect(jobPercent(job({ status: "running", progress: 41.6 }))).toBe(42);
    expect(jobPercent(job({ status: "running", progress: 250 }))).toBe(100);
    expect(jobPercent(job({ status: "running", progress: -3 }))).toBe(0);
    expect(jobPercent(job({ status: "running", progress: null }))).toBeUndefined();
    expect(jobPercent(job({ status: "running" }))).toBeUndefined();
    for (const status of ["queued", "succeeded", "failed", "cancel_requested", "cancelled"]) {
      expect(jobPercent(job({ status, progress: 60 }))).toBeUndefined();
    }
  });
});

describe("runningFor", () => {
  it("reads the time since a start and says nothing without one", () => {
    const now = new Date("2026-07-20T10:02:14Z");
    expect(runningFor("2026-07-20T10:00:00Z", now)).toBe("2 min 14 s");
    expect(runningFor("2026-07-20T10:02:00Z", now)).toBe("14 s");
    expect(runningFor(null, now)).toBe("");
    expect(runningFor(undefined, now)).toBe("");
    expect(runningFor("2026-07-20T11:00:00Z", now)).toBe("");
  });
});

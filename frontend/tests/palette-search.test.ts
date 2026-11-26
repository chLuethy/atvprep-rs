import { describe, expect, it, vi } from "vitest";
import { searchTargets } from "../src/features/search/paletteSearch";

const json = (data: unknown) => new Response(JSON.stringify({ data }), { status: 200 });
const fakeApi = (routes: Record<string, unknown>) => ({
  request: vi.fn(async (path: string) => {
    const key = Object.keys(routes).find((prefix) => path.startsWith(prefix));
    return key ? json(routes[key]) : new Response("{}", { status: 404 });
  }),
});

const FILE = { id: "m_7", display_path: "TV/Show/Season 01/The Marvelous Mrs. Maisel (2017) - S01E02 - Titel [AMZN].mp4" };
const JOB = { id: "j_1", action: "verify", subject: "Failed Movie.mp4", total_count: 1 };

describe("searchTargets", () => {
  it("asks the existing read paths and opens the file, the job and the schedule", async () => {
    const api = fakeApi({
      "/files?": [FILE],
      "/jobs?": [JOB],
      "/schedules": [{ id: "s_1", name: "Nightly Maisel check", action: "verify" }, { id: "s_2", name: "Other", action: "scan" }],
    });
    const go = vi.fn();

    const hits = await searchTargets(api, "maisel", go);

    expect(api.request.mock.calls.map(([path]) => path)).toEqual([expect.stringContaining("/files?"), expect.stringContaining("/jobs?"), "/schedules"]);
    expect(api.request.mock.calls[0]![0]).toContain("q=maisel");
    expect(hits.map((hit) => [hit.label, hit.hint])).toEqual([
      ["The Marvelous Mrs. Maisel (2017) - S01E02 - Titel [AMZN]", "File"],
      ["Failed Movie", "Job"],
      ["Nightly Maisel check", "Schedule"],
    ]);
    hits.forEach((hit) => hit.run());
    expect(go.mock.calls.map(([path]) => path)).toEqual(["/files/m_7", "/jobs/j_1", "/schedules"]);
  });

  it("returns what the other reads found when one fails", async () => {
    const api = fakeApi({ "/files?": [FILE] });

    const hits = await searchTargets(api, "S01E02", vi.fn());

    expect(hits).toHaveLength(1);
  });
});

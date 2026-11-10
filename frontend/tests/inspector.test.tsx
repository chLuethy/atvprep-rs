import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Inspector } from "../src/features/files/Inspector";
import { SessionError } from "../src/lib/api";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };

const detail = {
  id: "m_f1",
  root_id: "r_1",
  display_path: "Film/Pretty Woman (1990)/Pretty Woman (1990).mp4",
  kind: "movie",
  suffix: ".mp4",
  size: 10 * 1024 ** 3,
  mtime_ns: 1,
  discovery_state: "present",
  compliance_state: "blocker",
  compliance_reasons: [
    "[blocker] ftyp-dby1-requires-atmos: ftyp lists dby1 but no track is Atmos",
    "[warning] apple-tags: 4 tracks lack an elng tag",
  ],
  workflow_state: "idle",
  desired_action: "fix",
  last_seen_at: "2026-07-20T10:00:00Z",
  status_version: 3,
  last_success_at: "2026-07-20T09:00:00Z",
  has_error: false,
  sidecar_count: 2,
  inspection_available: true,
};

const inspection = {
  id: "i_1",
  media_id: "m_f1",
  tool_name: "ffprobe",
  tool_version: "6.1",
  result: {
    format: { format_name: "mov,mp4,m4a,3gp,3g2,mj2", duration: "7177.0", bit_rate: "12129150" },
    streams: [
      { index: 0, codec_type: "video", codec_name: "h264", width: 1920, height: 1080, disposition: { default: 1 }, tags: { language: "eng" } },
      { index: 1, codec_type: "audio", codec_name: "eac3", channels: 6, disposition: { default: 1 }, tags: { language: "deu" } },
      { index: 2, codec_type: "audio", codec_name: "aac", channels: 2, disposition: { default: 0 }, tags: { language: "deu" } },
      { index: 3, codec_type: "subtitle", codec_name: "mov_text", disposition: { forced: 1 }, tags: { language: "deu" } },
    ],
  },
  created_at: "2026-07-20T08:00:00Z",
};

const job = {
  id: "j_1", action: "inspect", status: "succeeded", trigger: "manual", origin_job_id: null,
  requested_at: "2026-07-20T07:00:00Z", started_at: null, finished_at: null,
  total_count: 1, succeeded_count: 1, failed_count: 0, cancelled_count: 0,
};

const sidecar = (id: string, name: string) => ({ id, media_id: "m_f1", name, size: 42, mtime_ns: 9, fingerprint: "fp", created_at: null });

function response(data: unknown, status = 200, meta: unknown = {}) {
  return new Response(JSON.stringify({ data, meta }), { status });
}

function apiFor(handler: (path: string, init?: RequestInit) => Response | Promise<Response>): Api {
  return { request: vi.fn<(path: string, init?: RequestInit) => Promise<Response>>(async (path, init) => handler(path, init)) };
}

/** Routes by exact path (a query string may follow); anything not listed answers an empty list. */
function standard(overrides: Record<string, () => Response | Promise<Response>> = {}) {
  return apiFor((path) => {
    for (const [route, make] of Object.entries(overrides)) if (path === route || path.startsWith(`${route}?`)) return make();
    if (path === "/files/m_f1") return response(detail);
    if (path.includes("/inspections")) return response([inspection]);
    if (path.includes("/jobs")) return response([job]);
    return response([]);
  });
}

afterEach(() => { vi.useRealTimers(); });

describe("Inspector: what the file is and why", () => {
  it("shows the title, folder, verdict and container facts", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);

    expect(await view.findByRole("heading", { name: "Pretty Woman (1990)" })).toBeInTheDocument();
    expect(view.getByText("Film/Pretty Woman (1990)")).toBeInTheDocument();
    expect(view.getByText("Blocker, fixable")).toBeInTheDocument();
    expect(await view.findByText("12.1 Mbps")).toBeInTheDocument();
    expect(view.getByText("1:59 h")).toBeInTheDocument();
    expect(view.getByText("10 GB")).toBeInTheDocument();
  });

  it("lists every verify reason with its severity and rule instead of a bare verdict", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);
    await view.findByRole("heading", { name: "Pretty Woman (1990)" });

    const why = view.getByRole("list", { name: "Why" });
    expect(why).toHaveTextContent("ftyp-dby1-requires-atmos");
    expect(why).toHaveTextContent("ftyp lists dby1 but no track is Atmos");
    expect(why).toHaveTextContent("apple-tags");
    expect(why).toHaveTextContent(/blocker/i);
    expect(why).toHaveTextContent(/warning/i);
  });

  it("explains that an unverified file is not known to be fine", async () => {
    const unknown = { ...detail, compliance_state: "unknown", compliance_reasons: ["Current ffprobe inspection is available, but AVPlayer compliance requires a verify report."] };
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response(unknown) })} fileId="m_f1" />);

    expect(await view.findByText("Not verified")).toBeInTheDocument();
    expect(view.getByText(/only a verify report can mark a file ready/i)).toBeInTheDocument();
  });

  it("states plainly when no reason is available", async () => {
    const bare = { ...detail, compliance_state: "warning", compliance_reasons: [] };
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response(bare) })} fileId="m_f1" />);

    expect(await view.findByText(/no reason recorded/i)).toBeInTheDocument();
  });

  it("offers Fix only for fixable files and reports both actions upward", async () => {
    const onFix = vi.fn();
    const onVerify = vi.fn();
    const view = render(() => <Inspector api={standard()} fileId="m_f1" onFix={onFix} onVerify={onVerify} />);

    await fireEvent.click(await view.findByRole("button", { name: /prepare preview/i }));
    await fireEvent.click(view.getByRole("button", { name: "Verify" }));

    expect(onFix).toHaveBeenCalledWith(expect.objectContaining({ id: "m_f1", compliance_state: expect.any(String) }));
    expect(onVerify).toHaveBeenCalledWith("m_f1");
    view.unmount();

    const clean = { ...detail, compliance_state: "clean", desired_action: "none", compliance_reasons: ["No compliance violations reported by verify."] };
    const cleanView = render(() => <Inspector api={standard({ "/files/m_f1": () => response(clean) })} fileId="m_f1" onFix={onFix} />);
    await cleanView.findAllByRole("heading", { name: "Pretty Woman (1990)" });
    expect(cleanView.queryByRole("button", { name: /prepare preview/i })).not.toBeInTheDocument();
  });
});

describe("Inspector: tracks and inspection", () => {
  it("renders one row per track with language, codec, layout and flags from the inspection", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);

    const table = await view.findByRole("table", { name: "Tracks" });
    const rows = Array.from(table.querySelectorAll("tbody tr")).map((row) => row.textContent ?? "");
    expect(rows).toHaveLength(4);
    expect(rows[0]).toContain("V0");
    expect(rows[0]).toContain("1920×1080");
    expect(rows[1]).toMatch(/A0.*German.*eac3.*5\.1.*Default/);
    expect(rows[2]).toMatch(/A1.*German.*aac.*Stereo/);
    expect(rows[3]).toMatch(/S0.*German.*mov_text.*Forced/);
  });

  it("keeps showing the tracks after a verify, whose report carries no streams", async () => {
    const verify = { id: "i_2", media_id: "m_f1", tool_name: "verify", tool_version: "atvprep", result: { passed: true, violations: [] }, created_at: "2026-07-20T11:00:00Z" };
    const view = render(() => <Inspector api={standard({ "/files/m_f1/inspections": () => response([verify, inspection]) })} fileId="m_f1" />);

    const table = await view.findByRole("table", { name: "Tracks" });

    expect(within(table).getAllByRole("row")).toHaveLength(5);
    expect(view.queryByText(/no tracks yet/i)).not.toBeInTheDocument();
    expect(view.getByText("ffprobe 6.1")).toBeInTheDocument();
  });

  it.each([["queued", "Prepare queued"], ["running", "Prepare running"], ["cancelling", "Prepare cancelling"]])(
    "shows %s instead of Fix preview and offers no second fix",
    async (fixStatus, label) => {
      const onFix = vi.fn();
      const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response({ ...detail, fix_status: fixStatus }) })} fileId="m_f1" onFix={onFix} />);

      const status = await view.findByRole("button", { name: label });

      expect(status).toBeDisabled();
      expect(view.queryByRole("button", { name: /prepare preview/i })).not.toBeInTheDocument();
      await fireEvent.click(status);
      expect(onFix).not.toHaveBeenCalled();
    },
  );

  it("follows an active fix and offers the preview again once it is over", async () => {
    vi.useFakeTimers();
    let fixStatus: string | null = "running";
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response({ ...detail, fix_status: fixStatus }) })} fileId="m_f1" onFix={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    expect(view.getByRole("button", { name: "Prepare running" })).toBeDisabled();

    fixStatus = null;
    await vi.advanceTimersByTimeAsync(4100);

    expect(view.getByRole("button", { name: /prepare preview/i })).toBeEnabled();
    expect(view.queryByRole("button", { name: "Prepare running" })).not.toBeInTheDocument();
  });

  it("leaves the page alone when a refresh brings the same data under new ids", async () => {
    vi.useFakeTimers();
    let serial = 0;
    const api = standard({ "/files/m_f1": () => response({ ...detail, id: `m_${++serial}`, fix_status: "running" }) });
    const view = render(() => <Inspector api={api} fileId="m_f1" onFix={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    const title = view.getByRole("heading", { name: "Pretty Woman (1990)" });
    const table = view.getByRole("table", { name: "Tracks" });
    const status = view.getByRole("button", { name: "Prepare running" });

    await vi.advanceTimersByTimeAsync(13000);

    expect(serial).toBeGreaterThan(2);
    expect(view.getByRole("heading", { name: "Pretty Woman (1990)" })).toBe(title);
    expect(view.getByRole("table", { name: "Tracks" })).toBe(table);
    expect(view.getByRole("button", { name: "Prepare running" })).toBe(status);
    expect(view.queryByText(/loading/i)).not.toBeInTheDocument();
  });

  it("rebuilds only what changed when a refresh does bring a change", async () => {
    vi.useFakeTimers();
    let fixStatus: string | null = "running";
    const api = standard({ "/files/m_f1": () => response({ ...detail, fix_status: fixStatus }) });
    const view = render(() => <Inspector api={api} fileId="m_f1" onFix={vi.fn()} />);
    await vi.advanceTimersByTimeAsync(0);
    const title = view.getByRole("heading", { name: "Pretty Woman (1990)" });
    const table = view.getByRole("table", { name: "Tracks" });

    fixStatus = null;
    await vi.advanceTimersByTimeAsync(4100);

    expect(view.getByRole("button", { name: /prepare preview/i })).toBeEnabled();
    expect(view.queryByRole("button", { name: "Prepare running" })).not.toBeInTheDocument();
    expect(view.getByRole("heading", { name: "Pretty Woman (1990)" })).toBe(title);
    expect(view.getByRole("table", { name: "Tracks" })).toBe(table);
  });

  it("puts Verify first once a fix is done, because the result is only known after it", async () => {
    const fixed = { ...detail, compliance_state: "unknown", desired_action: "none", compliance_reasons: ["Fixed by atvprep. Verify to see the result."] };
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response(fixed) })} fileId="m_f1" onFix={vi.fn()} onVerify={vi.fn()} />);

    const verify = await view.findByRole("button", { name: "Verify" });

    expect(verify).toHaveClass("primary");
    view.unmount();

    const other = render(() => <Inspector api={standard()} fileId="m_f1" onFix={vi.fn()} onVerify={vi.fn()} />);
    expect(await other.findByRole("button", { name: "Verify" })).not.toHaveClass("primary");
  });

  it("says why there is no track table when nothing has been inspected", async () => {
    const view = render(() => <Inspector api={standard({ "/files/m_f1/inspections": () => response([]) })} fileId="m_f1" />);

    expect(await view.findByText(/no tracks yet/i)).toBeInTheDocument();
    expect(view.queryByRole("table", { name: "Tracks" })).not.toBeInTheDocument();
  });

  it("shows provenance and flags a stale inspection", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);

    expect(await view.findByText("ffprobe 6.1")).toBeInTheDocument();
    expect(view.getByText(/stale inspection/i)).toBeInTheDocument();
  });

  it("states when inspection freshness cannot be determined", async () => {
    const view = render(() => <Inspector api={standard({
      "/files/m_f1/inspections": () => response([{ ...inspection, created_at: null }]),
      "/files/m_f1": () => response({ ...detail, last_success_at: null }),
    })} fileId="m_f1" />);

    expect(await view.findByText(/freshness unavailable/i)).toBeInTheDocument();
  });

  it("keeps the raw ffprobe JSON one click away", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);

    await fireEvent.click(await view.findByText("Raw JSON"));
    expect(view.getByText(/"format_name": "mov,mp4,m4a,3gp,3g2,mj2"/)).toBeInTheDocument();
  });

  it("points to verify for Apple tag status instead of guessing it", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);
    await view.findByRole("table", { name: "Tracks" });

    expect(view.getByText(/apple tag status .* comes from a verify report and is listed under why/i)).toBeInTheDocument();
  });
});

describe("Inspector: playback in Wave", () => {
  afterEach(() => window.localStorage.clear());

  it("compares the bitrate with the Wave limit and says what a fix cannot change", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);
    const section = await view.findByRole("region", { name: "Playback in Wave" });

    expect(within(section).getByText("Within your limit")).toBeInTheDocument();
    await fireEvent.change(within(section).getByLabelText("Your Wave limit"), { target: { value: "10" } });

    expect(within(section).getByText("Wave transcodes this file")).toBeInTheDocument();
    expect(section).toHaveTextContent("12.1 Mbps is over your 10 Mbps limit");
    expect(section).toHaveTextContent(/a fix does not change this/i);
  });

  it("remembers the chosen limit", async () => {
    const first = render(() => <Inspector api={standard()} fileId="m_f1" />);
    await fireEvent.change(await first.findByLabelText("Your Wave limit"), { target: { value: "20" } });
    first.unmount();

    const second = render(() => <Inspector api={standard()} fileId="m_f1" />);

    expect(await second.findByLabelText("Your Wave limit")).toHaveValue("20");
  });

  it("does not judge a file that has not been inspected", async () => {
    const view = render(() => <Inspector api={standard({ "/files/m_f1/inspections": () => response([]) })} fileId="m_f1" />);

    const section = await view.findByRole("region", { name: "Playback in Wave" });
    expect(within(section).getByText("Bitrate unknown")).toBeInTheDocument();
  });
});

describe("Inspector: sidecars and history", () => {
  it("lists sidecars and pages through them", async () => {
    const api = apiFor((path) => {
      if (path === "/files/m_f1") return response(detail);
      if (path === "/files/m_f1/sidecars?limit=50") return response([sidecar("c_1", "movie.en.srt")], 200, { next_cursor: "cursor-2" });
      if (path === "/files/m_f1/sidecars?limit=50&cursor=cursor-2") return response([sidecar("c_2", "movie.fr.srt")]);
      return response([]);
    });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);

    expect(await view.findByText("movie.en.srt")).toBeInTheDocument();
    await fireEvent.click(view.getByRole("button", { name: /load more sidecars/i }));
    expect(await view.findByText("movie.fr.srt")).toBeInTheDocument();
    expect(api.request).toHaveBeenCalledWith("/files/m_f1/sidecars?limit=50&cursor=cursor-2");
  });

  it("falls back to the sidecar count and to an empty job history", async () => {
    const view = render(() => <Inspector api={standard({ "/files/m_f1/jobs": () => response([]) })} fileId="m_f1" />);

    expect(await view.findByText("2 sidecars available")).toBeInTheDocument();
    expect(await view.findByText(/no job history/i)).toBeInTheDocument();
  });

  it("lists job history with action, status and trigger", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);

    expect(await view.findByText("inspect: succeeded (manual)")).toBeInTheDocument();
  });
});

describe("Inspector: loading, errors, navigation", () => {
  it("retries a failed detail request", async () => {
    let failed = true;
    const api = apiFor((path) => {
      if (path === "/files/m_f1" && failed) { failed = false; return response({}, 503); }
      return path === "/files/m_f1" ? response(detail) : response([]);
    });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);

    expect(await view.findByRole("alert")).toHaveTextContent(/unable to load file detail/i);
    await fireEvent.click(view.getByRole("button", { name: "Retry" }));
    expect(await view.findByRole("heading", { name: "Pretty Woman (1990)" })).toBeInTheDocument();
  });

  it("shows a generic session-expired alert", async () => {
    const view = render(() => <Inspector api={apiFor(() => Promise.reject(new SessionError()))} fileId="m_f1" />);

    expect((await view.findAllByRole("alert")).some((alert) => /session expired.*lock.*unlock/i.test(alert.textContent ?? ""))).toBe(true);
  });

  it("reloads when the selected file changes", async () => {
    const api = apiFor((path) =>
      path === "/files/m_f1" ? response(detail)
        : path === "/files/m_f2" ? response({ ...detail, id: "m_f2", display_path: "Film/Heat (1995)/Heat.mp4" })
          : response([]));
    const [fileId, setFileId] = createSignal("m_f1");
    const view = render(() => <Inspector api={api} fileId={fileId()} />);

    expect(await view.findByRole("heading", { name: "Pretty Woman (1990)" })).toBeInTheDocument();
    setFileId("m_f2");
    expect(await view.findByRole("heading", { name: "Heat" })).toBeInTheDocument();
  });
});

describe("Inspector: re-inspect", () => {
  const inspectPosts = (api: Api) => (api.request as ReturnType<typeof vi.fn>).mock.calls.filter(([path]) => path === "/files/m_f1/inspect");

  it("asks for confirmation inline, not in a modal, and queues once", async () => {
    const api = standard({ "/files/m_f1/inspect": () => new Response(JSON.stringify({ data: { id: "j_2" } }), { status: 201 }) });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);

    await fireEvent.click(await view.findByRole("button", { name: "Re-inspect" }));
    expect(view.queryByRole("dialog")).not.toBeInTheDocument();
    const region = view.getByRole("region", { name: "Confirm re-inspection" });
    expect(region).toHaveTextContent(/queue a new ffprobe inspection/i);
    expect(inspectPosts(api)).toHaveLength(0);

    await fireEvent.click(view.getByRole("button", { name: "Confirm re-inspect" }));
    expect(await view.findByText(/re-inspection queued/i)).toBeInTheDocument();
    expect(inspectPosts(api)).toHaveLength(1);
    expect(view.queryByRole("region", { name: "Confirm re-inspection" })).not.toBeInTheDocument();
  });

  it("blocks a second submit while the first is in flight", async () => {
    const api = standard({ "/files/m_f1/inspect": () => new Promise<Response>((resolve) => setTimeout(() => resolve(response({ id: "j_2" }, 201)), 10)) });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);
    await fireEvent.click(await view.findByRole("button", { name: "Re-inspect" }));

    const confirm = view.getByRole("button", { name: "Confirm re-inspect" });
    await fireEvent.click(confirm);
    expect(confirm).toBeDisabled();
    await fireEvent.click(confirm);

    await view.findByText(/re-inspection queued/i);
    expect(inspectPosts(api)).toHaveLength(1);
  });

  it("reuses one idempotency key when the same confirmation is retried", async () => {
    let posts = 0;
    const api = standard({ "/files/m_f1/inspect": () => (++posts === 1 ? response({}, 503) : response({ id: "j_2" }, 201)) });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);
    await fireEvent.click(await view.findByRole("button", { name: "Re-inspect" }));

    await fireEvent.click(view.getByRole("button", { name: "Confirm re-inspect" }));
    await view.findByRole("alert");
    await fireEvent.click(view.getByRole("button", { name: "Confirm re-inspect" }));
    await waitFor(() => expect(posts).toBe(2));

    const [first, second] = inspectPosts(api);
    const firstKey = new Headers(first![1]?.headers).get("Idempotency-Key");
    expect(firstKey).toBeTruthy();
    expect(new Headers(second![1]?.headers).get("Idempotency-Key")).toBe(firstKey);
  });

  it("moves focus to Confirm, cancels on Escape and hands focus back to the trigger", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);
    const trigger = await view.findByRole("button", { name: "Re-inspect" });
    trigger.focus();
    await fireEvent.click(trigger);

    const confirm = view.getByRole("button", { name: "Confirm re-inspect" });
    await waitFor(() => expect(document.activeElement).toBe(confirm));
    await fireEvent.keyDown(view.getByRole("region", { name: "Confirm re-inspection" }), { key: "Escape" });

    await waitFor(() => expect(view.queryByRole("region", { name: "Confirm re-inspection" })).not.toBeInTheDocument());
    expect(document.activeElement).toBe(trigger);
  });

  it("reports an expired session and closes the confirmation", async () => {
    const api = standard({ "/files/m_f1/inspect": () => Promise.reject(new SessionError()) });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);
    await fireEvent.click(await view.findByRole("button", { name: "Re-inspect" }));
    await fireEvent.click(view.getByRole("button", { name: "Confirm re-inspect" }));

    await expect(view.findByRole("alert")).resolves.toHaveTextContent(/session expired.*lock.*unlock/i);
    expect(view.queryByRole("region", { name: "Confirm re-inspection" })).not.toBeInTheDocument();
  });
});

describe("Inspector: what the prepare confirmation says", () => {
  const mkv = { ...detail, display_path: "Serie/Lynley.mkv", suffix: ".mkv", compliance_state: "unknown", compliance_reasons: [] };

  it("keeps the original while the setting is off", async () => {
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response(mkv) })} fileId="m_f1" deleteOriginal={false} />);
    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));
    const region = await view.findByRole("region", { name: "Prepare" });
    expect(region).toHaveTextContent("The original is kept.");
    expect(region).not.toHaveTextContent(/delet/i);
  });

  it("says the original is removed after verification while the setting is on", async () => {
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response(mkv) })} fileId="m_f1" deleteOriginal />);
    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));
    expect(await view.findByRole("region", { name: "Prepare" })).toHaveTextContent("The original is removed once the new MP4 passes verification.");
  });
});

describe("Inspector: converting a file that is not an MP4", () => {
  const mkvDetail = { ...detail, display_path: "Serie/Lynley/Lynley S01E01.mkv", suffix: ".mkv", compliance_state: "unknown", compliance_reasons: [] };
  const mkvInspection = (video: string, height = 1080, audio = "eac3") => ({
    ...inspection,
    result: { format: { format_name: "matroska,webm" }, streams: [
      { index: 0, codec_type: "video", codec_name: video, width: 1920, height },
      { index: 1, codec_type: "audio", codec_name: audio, channels: 6 },
    ] },
  });
  const forMkv = (video: string, height?: number, audio?: string, extra: Record<string, () => Response | Promise<Response>> = {}, need: Record<string, unknown> = { encode_need: "none", planned_change: "remux" }) =>
    standard({ "/files/m_f1": () => response({ ...mkvDetail, ...need }), "/files/m_f1/inspections": () => response([mkvInspection(video, height, audio)]), ...extra });

  it("offers Convert instead of Verify and Fix, and a remux needs no long-running warning", async () => {
    const view = render(() => <Inspector api={forMkv("h264")} fileId="m_f1" onVerify={() => {}} onFix={() => {}} />);

    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));

    expect(view.queryByRole("button", { name: "Verify" })).not.toBeInTheDocument();
    const confirm = await view.findByRole("region", { name: "Prepare" });
    await waitFor(() => expect(confirm).toHaveTextContent("Prepare writes an MP4 next to this .mkv file"));
    expect(within(confirm).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows need and change from the server and predicts nothing from the codecs", async () => {
    // vp9 used to read as a predicted re-encode; the server says otherwise here.
    const view = render(() => <Inspector api={forMkv("vp9")} fileId="m_f1" />);

    expect(await view.findByText("No encoding")).toBeInTheDocument();
    expect(view.getByText("Remux")).toBeInTheDocument();
    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));
    const confirm = await view.findByRole("region", { name: "Prepare" });
    expect(within(confirm).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("offers a file with an unknown need and says the run keeps to the level", async () => {
    const view = render(() => <Inspector api={forMkv("h264", 1080, "eac3", {}, { encode_need: "unknown", planned_change: "unknown" })} fileId="m_f1" />);

    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));
    const confirm = await view.findByRole("region", { name: "Prepare" });
    await waitFor(() => expect(confirm).toHaveTextContent(/not known yet/));
  });

  it("offers the allowance, audio preselected, and confirms the video level before it queues", async () => {
    const posted: unknown[] = [];
    const api = forMkv("vp9", 1080, "dts", { "/jobs": () => response({ id: "j_9" }, 201) }, { encode_need: "video", planned_change: "reencode" });
    const original = api.request;
    api.request = vi.fn(async (path: string, init?: RequestInit) => {
      if (path === "/jobs") posted.push(JSON.parse(String(init?.body)));
      return original(path, init);
    });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);

    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));
    const confirm = await view.findByRole("region", { name: "Prepare" });
    await waitFor(() => expect(within(confirm).getByRole("alert")).toHaveTextContent(/has to be re-encoded/));
    expect(within(confirm).getByRole("radio", { name: /^Encode audio\b(?! and)/ })).toBeChecked();
    expect(posted).toEqual([]);

    await fireEvent.click(within(confirm).getByRole("radio", { name: /^Encode audio and video/ }));
    expect(within(confirm).getByRole("button", { name: "Confirm prepare" })).toBeDisabled();
    await fireEvent.click(within(confirm).getByRole("checkbox", { name: /re-encoded/ }));
    await fireEvent.click(within(confirm).getByRole("button", { name: "Confirm prepare" }));
    await waitFor(() => expect(posted).toEqual([{ action: "prepare", media_ids: ["m_f1"], manual_approval: true, encode: "all", confirm_video_encode: true }]));
  });

  it("queues with the preselected Encode audio without a further step", async () => {
    const posted: unknown[] = [];
    const api = forMkv("h264", 1080, "eac3", { "/jobs": () => response({ id: "j_9" }, 201) });
    const original = api.request;
    api.request = vi.fn(async (path: string, init?: RequestInit) => {
      if (path === "/jobs") posted.push(JSON.parse(String(init?.body)));
      return original(path, init);
    });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);

    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));
    await fireEvent.click(within(await view.findByRole("region", { name: "Prepare" })).getByRole("button", { name: "Confirm prepare" }));
    await waitFor(() => expect(posted).toEqual([{ action: "prepare", media_ids: ["m_f1"], manual_approval: true, encode: "audio" }]));
  });

  it("keeps Verify and Fix for MP4 files and never offers Convert there", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" onVerify={() => {}} onFix={() => {}} />);

    expect(await view.findByRole("button", { name: "Verify" })).toBeInTheDocument();
    expect(view.queryByRole("button", { name: "Prepare" })).not.toBeInTheDocument();
  });
});

describe("Inspector: the convert confirmation", () => {
  it("says the original is kept after a conversion", async () => {
    const mkv = { ...detail, display_path: "Serie/Lynley.mkv", suffix: ".mkv", compliance_state: "unknown", compliance_reasons: [] };
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response(mkv) })} fileId="m_f1" deleteOriginal={false} />);

    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));

    expect(await view.findByRole("region", { name: "Prepare" })).toHaveTextContent("The original is kept.");
  });
});

describe("Inspector: a refused conversion", () => {
  it("shows the reason the server gave", async () => {
    const mkv = { ...detail, display_path: "Serie/Lynley.mkv", suffix: ".mkv", compliance_state: "unknown", compliance_reasons: [] };
    const api = standard({
      "/files/m_f1": () => response(mkv),
      "/jobs": () => new Response(JSON.stringify({ detail: "transcode jobs are disabled; only manual fix jobs are supported" }), { status: 422 }),
    });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);

    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));
    await fireEvent.click(await view.findByRole("button", { name: "Confirm prepare" }));

    expect(await view.findByText(/The server said: transcode jobs are disabled/)).toBeInTheDocument();
  });
});

describe("Inspector: the id sent with a conversion", () => {
  it("is the id from the file the server sent, not the one in the address", async () => {
    const mkv = { ...detail, id: "m_RAW==", display_path: "Serie/Lynley.mkv", suffix: ".mkv", compliance_state: "unknown", compliance_reasons: [] };
    let body: unknown;
    const api = apiFor((path, init) => {
      if (path === "/jobs") { body = JSON.parse(String(init?.body)); return response({ id: "j_1" }, 201); }
      if (path === "/files/m_RAW%3D%3D") return response(mkv);
      return response([]);
    });
    const view = render(() => <Inspector api={api} fileId="m_RAW%3D%3D" />);

    await fireEvent.click(await view.findByRole("button", { name: "Prepare" }));
    await fireEvent.click(await view.findByRole("button", { name: "Confirm prepare" }));

    await waitFor(() => expect(body).toEqual({ action: "prepare", media_ids: ["m_RAW=="], manual_approval: true, encode: "audio" }));
  });
});

describe("Inspector: external subtitle files", () => {
  const sidecarRow = (id: string, name: string, state: string, language: string) => ({ id, media_id: "m_f1", name, size: 42, mtime_ns: 9, fingerprint: "fp", created_at: null, state, language });

  it("says what a fix does with each one and offers the fix for a ready file", async () => {
    const ready = { ...detail, compliance_state: "clean", compliance_reasons: [], desired_action: "none", pending_sidecars: 1 };
    const api = standard({
      "/files/m_f1": () => response(ready),
      "/files/m_f1/sidecars": () => response([
        sidecarRow("c_1", "Film.de.srt", "pending", "deu"),
        sidecarRow("c_2", "Film.srt", "unknown_language", "und"),
        sidecarRow("c_3", "Film.fr.srt", "ignored_language", "fra"),
      ]),
    });
    const view = render(() => <Inspector api={api} fileId="m_f1" onFix={() => {}} />);

    const list = await view.findByRole("list", { name: "Sidecar files" });
    expect(list).toHaveTextContent("German: Prepare embeds it");
    expect(list).toHaveTextContent("Language not recognised from the name");
    expect(list).toHaveTextContent("only German, English and the audio languages are kept");
    expect(view.getByRole("button", { name: /Prepare preview/ })).toBeInTheDocument();
  });
  it("reads the sidecars again when a job ends, so a finished re-inspection shows its outcome", async () => {
    let state = "unchecked";
    const api = standard({
      "/files/m_f1/sidecars": () => response([sidecarRow("c_1", "Film.en.srt", state, "eng")]),
    });
    const [version, setVersion] = createSignal(0);
    const view = render(() => <Inspector api={api} fileId="m_f1" version={version()} />);

    const list = await view.findByRole("list", { name: "Sidecar files" });
    expect(list).toHaveTextContent("English: not known yet whether Prepare embeds it");

    state = "embedded";
    setVersion(1);

    await waitFor(() => expect(view.getByRole("list", { name: "Sidecar files" })).toHaveTextContent("English: not used, the file already has this subtitle"));
  });
});

describe("Inspector: an interrupted preparation", () => {
  it("notes the interruption and starts the normal manual preparation again, never on its own", async () => {
    const onFix = vi.fn();
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response({ ...detail, last_interrupted: true }) })} fileId="m_f1" onFix={onFix} />);

    expect(await view.findByText("Last preparation interrupted")).toBeInTheDocument();
    expect(onFix).not.toHaveBeenCalled();
    await fireEvent.click(view.getByRole("button", { name: "Prepare again" }));
    expect(onFix).toHaveBeenCalledTimes(1);
  });

  it("shows nothing when the last preparation was not interrupted", async () => {
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response({ ...detail, last_interrupted: false }) })} fileId="m_f1" onFix={vi.fn()} />);

    await view.findByRole("heading", { name: "Pretty Woman (1990)" });
    expect(view.queryByText("Last preparation interrupted")).not.toBeInTheDocument();
    expect(view.queryByRole("button", { name: "Prepare again" })).not.toBeInTheDocument();
  });
});

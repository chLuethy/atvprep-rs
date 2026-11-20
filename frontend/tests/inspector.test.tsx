import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Inspector } from "../src/features/files/Inspector";
import { SessionError } from "../src/lib/api";
import { withServerState } from "./fakeServerState";

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
  return new Response(JSON.stringify({ data: withServerState(data), meta }), { status });
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

  it("lists only the findings as reasons, the planner's steps come apart from them", async () => {
    const planned = { ...detail, compliance_state: "warning", compliance_reasons: [
      "[warning] audio-sample-rate: ac-3 track id=2 sample rate 44100Hz (expected 48000)",
    ], planned_steps: [{ kind: "sample_rate", stream: 1, from_codec: "ac3", to_codec: "eac3", from_hz: 44100, to_hz: 48000 }] };
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response(planned) })} fileId="m_f1" />);
    await view.findByRole("heading", { name: "Pretty Woman (1990)" });

    const why = view.getByRole("list", { name: "Why" });
    expect(within(why).getAllByRole("listitem")).toHaveLength(1);
    expect(why).not.toHaveTextContent("kHz");
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

  it("has no action buttons but Copy path: Verify, Re-inspect and Prepare live in the selection bar", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);
    await view.findAllByRole("heading", { name: "Pretty Woman (1990)" });

    expect(view.getAllByRole("button").map((button) => button.textContent)).toEqual(["Copy path"]);
  });

  it("copies the path of the file", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);

    await fireEvent.click(await view.findByRole("button", { name: "Copy path" }));

    expect(writeText).toHaveBeenCalledWith("Film/Pretty Woman (1990)/Pretty Woman (1990).mp4");
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
    "says %s in words while a preparation is going, without a button for it",
    async (fixStatus, label) => {
      const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response({ ...detail, fix_status: fixStatus }) })} fileId="m_f1" />);

      expect(await view.findByText(label)).toBeInTheDocument();
      expect(view.queryByRole("button", { name: label })).not.toBeInTheDocument();
    },
  );

  it("follows an active fix and drops the note once it is over", async () => {
    vi.useFakeTimers();
    let fixStatus: string | null = "running";
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response({ ...detail, fix_status: fixStatus }) })} fileId="m_f1" />);
    await vi.advanceTimersByTimeAsync(0);
    expect(view.getByText("Prepare running")).toBeInTheDocument();

    fixStatus = null;
    await vi.advanceTimersByTimeAsync(4100);

    expect(view.queryByText("Prepare running")).not.toBeInTheDocument();
  });

  it("zeigt Sitzungsverlust bei stiller Revalidierung statt vorhandene Details als Erfolg zu behalten", async () => {
    vi.useFakeTimers();
    let expire!: (reason: Error) => void;
    let reads = 0;
    const api = standard({ "/files/m_f1": () => ++reads === 1 ? response({ ...detail, fix_status: "running" })
      : new Promise<Response>((_resolve, reject) => { expire = reject; }) });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(4100);
    expect(view.getByRole("heading", { name: "Pretty Woman (1990)" })).toBeInTheDocument();
    expect(view.queryByText("Loading file detail")).not.toBeInTheDocument();
    expire(new SessionError());
    await vi.advanceTimersByTimeAsync(0);
    expect(view.getByRole("alert")).toHaveTextContent("Session expired. Lock and unlock again.");
    expect(view.queryByRole("heading", { name: "Pretty Woman (1990)" })).not.toBeInTheDocument();
    view.unmount();
  });

  it("leaves the page alone when a refresh brings the same data under new ids", async () => {
    vi.useFakeTimers();
    let serial = 0;
    const api = standard({ "/files/m_f1": () => response({ ...detail, id: `m_${++serial}`, fix_status: "running" }) });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);
    await vi.advanceTimersByTimeAsync(0);
    const title = view.getByRole("heading", { name: "Pretty Woman (1990)" });
    const table = view.getByRole("table", { name: "Tracks" });
    const status = view.getByText("Prepare running");

    await vi.advanceTimersByTimeAsync(13000);

    expect(serial).toBeGreaterThan(2);
    expect(view.getByRole("heading", { name: "Pretty Woman (1990)" })).toBe(title);
    expect(view.getByRole("table", { name: "Tracks" })).toBe(table);
    expect(view.getByText("Prepare running")).toBe(status);
    expect(view.queryByText(/loading/i)).not.toBeInTheDocument();
  });

  it("rebuilds only what changed when a refresh does bring a change", async () => {
    vi.useFakeTimers();
    let fixStatus: string | null = "running";
    const api = standard({ "/files/m_f1": () => response({ ...detail, fix_status: fixStatus }) });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);
    await vi.advanceTimersByTimeAsync(0);
    const title = view.getByRole("heading", { name: "Pretty Woman (1990)" });
    const table = view.getByRole("table", { name: "Tracks" });

    fixStatus = null;
    await vi.advanceTimersByTimeAsync(4100);

    expect(view.queryByText("Prepare running")).not.toBeInTheDocument();
    expect(view.getByRole("heading", { name: "Pretty Woman (1990)" })).toBe(title);
    expect(view.getByRole("table", { name: "Tracks" })).toBe(table);
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

  it("compares the bitrate with the Wave limit and says what preparing cannot change", async () => {
    const view = render(() => <Inspector api={standard()} fileId="m_f1" />);
    const section = await view.findByRole("region", { name: "Playback in Wave" });

    expect(within(section).getByText("Within your limit")).toBeInTheDocument();
    await fireEvent.change(within(section).getByLabelText("Your Wave limit"), { target: { value: "10" } });

    expect(within(section).getByText("Wave transcodes this file")).toBeInTheDocument();
    expect(section).toHaveTextContent("12.1 Mbps is over your 10 Mbps limit");
    expect(section).toHaveTextContent(/prepare does not change this/i);
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

  it("lädt beim synchronen Doppelklick nur eine weitere Untertitelseite", async () => {
    let finish!: (value: Response) => void;
    let reads = 0;
    const api = apiFor((path) => {
      if (path === "/files/m_f1") return response(detail);
      if (path.includes("cursor=next")) { reads += 1; return new Promise<Response>((resolve) => { finish = resolve; }); }
      if (path.startsWith("/files/m_f1/sidecars")) return response([sidecar("c_1", "first.srt")], 200, { next_cursor: "next" });
      return response([]);
    });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);
    const more = await view.findByRole("button", { name: "Load more sidecars" });
    fireEvent.click(more);
    fireEvent.click(more);
    expect(reads).toBe(1);
    finish(response([sidecar("c_2", "second.srt")]));
    expect(await view.findByText("second.srt")).toBeInTheDocument();
    expect(view.getByText("first.srt")).toBeInTheDocument();
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
  it("zeigt nach einem Dateiwechsel keine verspäteten Details, Inspektionen oder Jobs der alten Datei", async () => {
    let finishDetail!: (value: Response) => void;
    let finishInspections!: (value: Response) => void;
    let finishJobs!: (value: Response) => void;
    const api = apiFor((path) => {
      if (path === "/files/m_f1") return new Promise<Response>((resolve) => { finishDetail = resolve; });
      if (path.startsWith("/files/m_f1/inspections")) return new Promise<Response>((resolve) => { finishInspections = resolve; });
      if (path.startsWith("/files/m_f1/jobs")) return new Promise<Response>((resolve) => { finishJobs = resolve; });
      if (path === "/files/m_f2") return response({ ...detail, id: "m_f2", display_path: "Film/Heat.mp4" });
      return response([]);
    });
    const [fileId, setFileId] = createSignal("m_f1");
    const view = render(() => <Inspector api={api} fileId={fileId()} />);
    await waitFor(() => expect(finishJobs).toBeTypeOf("function"));

    setFileId("m_f2");
    await view.findByRole("heading", { name: "Heat" });
    finishDetail(response(detail));
    finishInspections(response([inspection]));
    finishJobs(response([job]));

    await waitFor(() => expect(view.getByRole("heading", { name: "Heat" })).toBeInTheDocument());
    expect(view.queryByRole("heading", { name: "Pretty Woman (1990)" })).not.toBeInTheDocument();
    expect(view.queryByText("12.1 Mbps")).not.toBeInTheDocument();
    expect(view.queryByText("inspect: succeeded (manual)")).not.toBeInTheDocument();
  });

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

describe("Inspector: external subtitle files", () => {
  const sidecarRow = (id: string, name: string, state: string, language: string) => ({ id, media_id: "m_f1", name, size: 42, mtime_ns: 9, fingerprint: "fp", created_at: null, state, language });

  it("says what a fix does with each one", async () => {
    const ready = { ...detail, compliance_state: "clean", compliance_reasons: [], desired_action: "none", pending_sidecars: 1 };
    const api = standard({
      "/files/m_f1": () => response(ready),
      "/files/m_f1/sidecars": () => response([
        sidecarRow("c_1", "Film.de.srt", "pending", "deu"),
        sidecarRow("c_2", "Film.srt", "unknown_language", "und"),
        sidecarRow("c_3", "Film.fr.srt", "ignored_language", "fra"),
      ]),
    });
    const view = render(() => <Inspector api={api} fileId="m_f1" />);

    const list = await view.findByRole("list", { name: "Sidecar files" });
    expect(list).toHaveTextContent("German: Prepare embeds it");
    expect(list).toHaveTextContent("Language not recognised from the name");
    expect(list).toHaveTextContent("only German, English and the audio languages are kept");
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
  it("notes the interruption in words and offers no button for it", async () => {
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response({ ...detail, last_interrupted: true }) })} fileId="m_f1" />);

    expect(await view.findByText("Last preparation interrupted")).toBeInTheDocument();
    expect(view.queryByRole("button", { name: "Prepare again" })).not.toBeInTheDocument();
  });

  it("shows nothing when the last preparation was not interrupted", async () => {
    const view = render(() => <Inspector api={standard({ "/files/m_f1": () => response({ ...detail, last_interrupted: false }) })} fileId="m_f1" />);

    await view.findByRole("heading", { name: "Pretty Woman (1990)" });
    expect(view.queryByText("Last preparation interrupted")).not.toBeInTheDocument();
  });
});

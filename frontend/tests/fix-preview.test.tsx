import { fireEvent, render, within } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";
import { FixPreview } from "../src/features/files/FixPreview";
import type { FileRow } from "../src/features/files/loaders";

const row = (over: Partial<FileRow> = {}): FileRow => ({
  id: "m_1", root_id: "r_1", display_path: "Film/Overboard (2018)/Overboard (2018).mp4", kind: "movie", suffix: ".mp4", size: 5 * 1024 ** 3,
  mtime_ns: 1, discovery_state: "present", compliance_state: "clean", workflow_state: "idle", desired_action: "none", last_seen_at: null,
  compliance_reasons: [], ...over,
});

const queued: string[] = [];
const show = (...files: FileRow[]) => render(() => <FixPreview files={files} skipped={0} onQueue={async (encode) => { queued.push(encode); return { ok: true }; }} onClose={() => {}} />);

describe("FixPreview: subtitles to embed", () => {
  it("names each subtitle that will be merged, with its language and kind", () => {
    const view = show(row({
      pending_sidecars: 3,
      embed_sidecars: [
        { name: "Overboard (2018).de.srt", language: "deu", kind: "normal" },
        { name: "Overboard (2018).en.hi.srt", language: "eng", kind: "sdh" },
        { name: "Overboard (2018).en.forced.srt", language: "eng", kind: "forced" },
      ],
    }));

    const list = view.getByRole("list", { name: "Subtitles to embed" });
    const items = within(list).getAllByRole("listitem").map((item) => item.textContent);
    expect(items).toEqual([
      "EmbedGerman subtitle from Overboard (2018).de.srt",
      "EmbedEnglish SDH subtitle from Overboard (2018).en.hi.srt",
      "EmbedEnglish forced subtitle from Overboard (2018).en.forced.srt",
    ]);
    expect(view.queryByText(/lists nothing more specific/)).not.toBeInTheDocument();
  });

  it("lists the subtitles next to the repairs the verify report asks for", () => {
    const view = show(row({
      compliance_state: "warning",
      compliance_reasons: ["[warning] apple-tags: 4 tracks lack an elng tag"],
      pending_sidecars: 1,
      embed_sidecars: [{ name: "Overboard (2018).de.srt", language: "deu", kind: "normal" }],
    }));

    expect(view.getByText("Write 4 elng language tags")).toBeInTheDocument();
    expect(view.getByText("German subtitle from Overboard (2018).de.srt")).toBeInTheDocument();
  });

  it("still says so when only a count is known", () => {
    const view = show(row({ pending_sidecars: 2 }));
    expect(view.getByText("Merge 2 external subtitle files into the MP4")).toBeInTheDocument();
  });

  it("mentions no subtitles when none are waiting", () => {
    const view = show(row({ compliance_state: "warning", compliance_reasons: ["[warning] apple-tags: 4 tracks lack an elng tag"] }));
    expect(view.queryByRole("list", { name: "Subtitles to embed" })).not.toBeInTheDocument();
  });
});

describe("FixPreview: the encoding allowance", () => {
  const warning = () => show(row({ compliance_state: "warning", compliance_reasons: ["[warning] apple-tags: 4 tracks lack an elng tag"] }));

  it("offers the three levels with Encode audio preselected and queues it without a further step", async () => {
    queued.length = 0;
    const view = warning();

    expect(view.getByRole("radio", { name: /^Copy only/ })).not.toBeChecked();
    expect(view.getByRole("radio", { name: /^Encode audio\b(?! and)/ })).toBeChecked();
    expect(view.getByRole("radio", { name: /^Encode audio and video/ })).not.toBeChecked();
    await fireEvent.click(view.getByRole("button", { name: "Prepare 1 file" }));
    expect(queued).toEqual(["audio"]);
  });

  it("queues Copy only when that is chosen", async () => {
    queued.length = 0;
    const view = warning();
    await fireEvent.click(view.getByRole("radio", { name: /^Copy only/ }));
    await fireEvent.click(view.getByRole("button", { name: "Prepare 1 file" }));
    expect(queued).toEqual(["copy"]);
  });

  it("holds Encode audio and video back until it is confirmed", async () => {
    queued.length = 0;
    const view = warning();
    expect(view.queryByRole("checkbox", { name: /re-encoded/ })).not.toBeInTheDocument();

    await fireEvent.click(view.getByRole("radio", { name: /^Encode audio and video/ }));
    expect(view.getByRole("button", { name: "Prepare 1 file" })).toBeDisabled();

    await fireEvent.click(view.getByRole("checkbox", { name: /re-encoded/ }));
    await fireEvent.click(view.getByRole("button", { name: "Prepare 1 file" }));
    expect(queued).toEqual(["all"]);
  });

  it("drops the confirmation when another level is chosen in between", async () => {
    const view = warning();
    await fireEvent.click(view.getByRole("radio", { name: /^Encode audio and video/ }));
    await fireEvent.click(view.getByRole("checkbox", { name: /re-encoded/ }));
    await fireEvent.click(view.getByRole("radio", { name: /^Copy only/ }));
    await fireEvent.click(view.getByRole("radio", { name: /^Encode audio and video/ }));
    expect(view.getByRole("button", { name: "Prepare 1 file" })).toBeDisabled();
  });

  it("shows the need and the change of each file from the server, and an AAC track is no remux", () => {
    const view = show(row({ encode_need: "audio", planned_change: "audio", compliance_state: "warning", compliance_reasons: ["[warning] stereo-twin: 1 language has surround only"] }));

    expect(view.getByText("Audio encoding")).toBeInTheDocument();
    expect(view.getByText(/Add an AAC stereo track/).previousElementSibling).toHaveTextContent("Encoding");
    expect(view.queryByText("Remux")).not.toBeInTheDocument();
  });

  it("counts a mixed selection per need", () => {
    const view = show(row({ id: "a", encode_need: "video" }), row({ id: "b", encode_need: "audio" }), row({ id: "c", encode_need: "audio" }), row({ id: "d" }));

    expect(view.getByLabelText("Encoding need of the selection")).toHaveTextContent("1 Video · 2 Audio · 1 Unknown");
  });

  it("warns about audio only under copy, and says when the need is not known", async () => {
    const view = show(row({ encode_need: "audio", compliance_state: "warning" }), row({ id: "u", encode_need: "unknown" }));

    expect(view.getByText(/1 of 2 files have no known encoding need/)).toBeInTheDocument();
    expect(view.queryByText(/needs? audio encoding/)).not.toBeInTheDocument();
    await fireEvent.click(view.getByRole("radio", { name: /^Copy only/ }));
    expect(view.getByText(/1 of 2 files need audio encoding/)).toBeInTheDocument();
  });

  it("names the files that need a re-encode until the video level is chosen", async () => {
    const view = show(row({ encode_need: "video", compliance_state: "warning" }));

    expect(view.getByText(/1 of 1 file needs a re-encode/)).toBeInTheDocument();
    expect(view.queryByRole("button", { name: /re-encode/i })).not.toBeInTheDocument();
    await fireEvent.click(view.getByRole("radio", { name: /^Encode audio and video/ }));
    expect(view.queryByText(/1 of 1 file needs a re-encode/)).not.toBeInTheDocument();
  });
});

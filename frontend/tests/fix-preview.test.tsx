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
const show = (file: FileRow) => render(() => <FixPreview files={[file]} skipped={0} onQueue={async (reencode) => { queued.push(reencode); return { ok: true }; }} onClose={() => {}} />);

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

describe("FixPreview: re-encode", () => {
  it("names how many files need a re-encode and offers both ways", async () => {
    queued.length = 0;
    const view = show(row({ needs_reencode: true, compliance_state: "warning" }));

    expect(view.getByText(/1 of 1 file needs a re-encode/)).toBeInTheDocument();
    expect(view.queryByRole("button", { name: "Prepare 1 file" })).not.toBeInTheDocument();
    await fireEvent.click(view.getByRole("button", { name: "With re-encode (1)" }));
    expect(queued).toEqual(["with"]);
  });

  it("offers a plain prepare when no file needs a re-encode", () => {
    const view = show(row({ compliance_state: "warning", compliance_reasons: ["[warning] apple-tags: 4 tracks lack an elng tag"] }));

    expect(view.getByRole("button", { name: "Prepare 1 file" })).toBeInTheDocument();
    expect(view.queryByRole("button", { name: /re-encode/i })).not.toBeInTheDocument();
  });
});

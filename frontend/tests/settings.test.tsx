import { fireEvent, render, waitFor, within } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { SettingsPage } from "../src/features/settings/SettingsPage";
import { SessionError } from "../src/lib/api";

type Api = { request: (path: string, init?: RequestInit) => Promise<Response> };
type Setting = { key: string; value: unknown; revision: number };
const response = (data: unknown, status = 200) => new Response(JSON.stringify({ data }), { status });
const settingsResponse = (revision = 7, settings: Setting[] = [
  { key: "scan_interval_seconds", value: 60, revision },
  { key: "settle_seconds", value: 120, revision },
  { key: "max_ffmpeg_workers", value: 4, revision },
  { key: "max_probe_workers", value: 2, revision },
  { key: "event_retention_days", value: 30, revision },
  { key: "server_version", value: "1.2.3", revision },
]) => response({ revision, settings });
const apiFor = (handler: (path: string, init?: RequestInit) => Response | Promise<Response>): Api => ({
  request: vi.fn(async (path, init) => handler(path, init)),
});

const audit = [{ key: "scan_interval_seconds", revision: 7, actor: "operator", old_value: 30, new_value: 60 }];

describe("audited runtime settings UI", () => {
  it("loads the governing runtime vocabulary and read-only structural values", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse() : response(audit));
    const view = render(() => <SettingsPage api={api} />);
    expect(await view.findByRole("heading", { name: "Runtime settings" })).toBeInTheDocument();
    expect(await view.findByLabelText("Scan interval (seconds)")).toHaveValue(60);
    expect(view.getByLabelText("Parallel conversions")).toHaveValue(4);
    expect(view.getByText("Revision 7")).toBeInTheDocument();
    expect(view.getByText(/operator/)).toBeInTheDocument();
    expect(view.getByLabelText("Server version")).toHaveValue("1.2.3");
    expect(view.getByLabelText("Server version")).toBeDisabled();
  });

  it("shows bounds errors and never sends invalid values", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse() : response([]));
    const view = render(() => <SettingsPage api={api} />);
    await view.findByLabelText("Scan interval (seconds)");
    fireEvent.input(view.getByLabelText("Scan interval (seconds)"), { target: { value: "86401" } });
    expect(view.getAllByText(/between 5 and 86400/i).length).toBeGreaterThan(0);
    expect(view.getByRole("button", { name: "Save settings" })).toBeDisabled();
  });

  it("edits the skipped folders one per line and saves them as a list", async () => {
    const api = apiFor((path, init) => {
      if (path === "/settings" && init?.method !== "PATCH") return settingsResponse(7, [{ key: "scan_exclude_dirs", value: ["Extras"], revision: 7 }]);
      if (path === "/settings/audit") return response([]);
      expect(JSON.parse(String(init?.body))).toEqual({ settings: { scan_exclude_dirs: ["Extras", "Film/Trailer"] }, expected_revision: 7 });
      return settingsResponse(8, [{ key: "scan_exclude_dirs", value: ["Extras", "Film/Trailer"], revision: 8 }]);
    });
    const view = render(() => <SettingsPage api={api} />);
    const field = (await view.findByLabelText("Folders the scan skips")) as HTMLTextAreaElement;
    expect(field.value).toBe("Extras");
    fireEvent.input(field, { target: { value: "Extras\nFilm/Trailer" } });
    fireEvent.click(view.getByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(view.getByText(/settings saved/i)).toBeInTheDocument());
  });

  it("says why a refused folder list was not saved", async () => {
    const api = apiFor((path, init) => {
      if (path === "/settings" && init?.method !== "PATCH") return settingsResponse(7, [{ key: "scan_exclude_dirs", value: [], revision: 7 }]);
      if (path === "/settings/audit") return response([]);
      return new Response(JSON.stringify({ detail: "invalid" }), { status: 422 });
    });
    const view = render(() => <SettingsPage api={api} />);
    fireEvent.input(await view.findByLabelText("Folders the scan skips"), { target: { value: "../x" } });
    fireEvent.click(view.getByRole("button", { name: "Save settings" }));
    expect(await view.findByRole("alert")).toHaveTextContent(/no leading \//i);
  });

  it("saves allowlisted values with the current revision and idempotency key", async () => {
    const api = apiFor((path, init) => {
      if (path === "/settings" && init?.method !== "PATCH") return settingsResponse();
      if (path === "/settings/audit") return response([]);
      expect(JSON.parse(String(init?.body))).toEqual({ settings: { scan_interval_seconds: 80 }, expected_revision: 7 });
      expect(new Headers(init?.headers).get("Idempotency-Key")).toMatch(/^settings-/);
      return settingsResponse(8, [{ key: "scan_interval_seconds", value: 80, revision: 8 }]);
    });
    const view = render(() => <SettingsPage api={api} />);
    await view.findByLabelText("Scan interval (seconds)");
    fireEvent.input(view.getByLabelText("Scan interval (seconds)"), { target: { value: "80" } });
    fireEvent.click(view.getByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(view.getByText(/settings saved/i)).toBeInTheDocument());
  });

  it("preserves the draft on server errors", async () => {
    const api = apiFor((path, init) => path === "/settings" && init?.method !== "PATCH" ? settingsResponse() : path === "/settings/audit" ? response([]) : response({ detail: "bad" }, 422));
    const view = render(() => <SettingsPage api={api} />);
    await view.findByLabelText("Scan interval (seconds)");
    fireEvent.input(view.getByLabelText("Scan interval (seconds)"), { target: { value: "80" } });
    fireEvent.click(view.getByRole("button", { name: "Save settings" }));
    expect(await view.findByRole("alert")).toHaveTextContent(/unable to save settings/i);
    expect(view.getByLabelText("Scan interval (seconds)")).toHaveValue(80);
  });

  it("preserves the draft on a revision conflict and supports reload/discard", async () => {
    let settingsCalls = 0;
    const api = apiFor((path, init) => {
      if (path === "/settings" && init?.method !== "PATCH") return ++settingsCalls === 1 ? settingsResponse() : settingsResponse(9, [{ key: "scan_interval_seconds", value: 120, revision: 9 }]);
      if (path.startsWith("/settings/audit")) return response([{ key: "scan_interval_seconds", revision: 9, actor: "other", old_value: 60, new_value: 120 }]);
      return response({}, 409);
    });
    const view = render(() => <SettingsPage api={api} />);
    await view.findByLabelText("Scan interval (seconds)");
    fireEvent.input(view.getByLabelText("Scan interval (seconds)"), { target: { value: "80" } });
    fireEvent.click(view.getByRole("button", { name: "Save settings" }));
    expect(await view.findByRole("alert")).toHaveTextContent(/changed these settings first/i);
    expect(view.getByLabelText("Scan interval (seconds)")).toHaveValue(80);
    fireEvent.click(view.getByRole("button", { name: "Use the current values" }));
    await waitFor(() => expect(view.getByLabelText("Scan interval (seconds)")).toHaveValue(120));
  });

  it("normalizes SessionError and avoids nested main landmarks", async () => {
    const api = apiFor(() => Promise.reject(new SessionError()));
    const view = render(() => <SettingsPage api={api} />);
    expect(await view.findByRole("alert")).toHaveTextContent(/session expired/i);
    expect(view.queryByRole("main")).not.toBeInTheDocument();
  });

  it("accepts revision zero and merges partial save responses", async () => {
    const api = apiFor((path, init) => {
      if (path === "/settings" && init?.method !== "PATCH") return settingsResponse(0);
      if (path === "/settings/audit") return response([]);
      return settingsResponse(1, [{ key: "scan_interval_seconds", value: 90, revision: 1 }]);
    });
    const view = render(() => <SettingsPage api={api} />);
    await view.findByLabelText("Scan interval (seconds)");
    fireEvent.input(view.getByLabelText("Scan interval (seconds)"), { target: { value: "90" } });
    fireEvent.click(view.getByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(view.getByText(/settings saved/i)).toBeInTheDocument());
    expect(view.getByLabelText("Parallel conversions")).toHaveValue(4);
    expect(view.getByLabelText("Server version")).toBeDisabled();
  });

  it("passes axe and covers labels, alerts, readonly values, audit, and controls", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse(3, [
      { key: "scan_interval_seconds", value: 60, revision: 2 },
      { key: "server_version", value: "1.2.3", revision: 0 },
    ]) : response(audit));
    const view = render(() => <SettingsPage api={api} />);
    const interval = await view.findByLabelText("Scan interval (seconds)");
    expect(interval).toHaveAccessibleName("Scan interval (seconds)");
    expect(view.getByLabelText("Server version")).toBeDisabled();
    expect(view.getByRole("region", { name: "Scan interval (seconds) history" })).toBeInTheDocument();
    fireEvent.input(interval, { target: { value: "86401" } });
    expect(view.getAllByRole("alert").length).toBeGreaterThan(0);
    expect(view.getByRole("button", { name: "Save settings" })).toBeDisabled();
    const results = await axe.run(view.container);
    expect(results.violations).toEqual([]);
  });

  it("groups settings by what they govern and keeps unknown keys in a read-only group", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse() : response([]));
    const view = render(() => <SettingsPage api={api} />);
    await view.findByLabelText("Scan interval (seconds)");

    expect(within(view.getByRole("region", { name: "Library scan" })).getByLabelText("Scan interval (seconds)")).toBeInTheDocument();
    expect(within(view.getByRole("region", { name: "Automation" })).getByLabelText("Parallel conversions")).toBeInTheDocument();
    expect(within(view.getByRole("region", { name: "Retention" })).getByLabelText("Keep events for (days)")).toBeInTheDocument();
    expect(within(view.getByRole("region", { name: "Other" })).getByLabelText("Server version")).toBeDisabled();
  });

  it("offers no switch that changes files: fixing, converting and deleting are never automatic", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse(7, [
      { key: "delete_original", value: true, revision: 7, default_value: false } as Setting,
    ]) : response([]));
    const view = render(() => <SettingsPage api={api} />);
    await view.findByRole("region", { name: "Other" });

    for (const label of ["Automatically fix MP4 files", "Apply new subtitle files automatically", "Convert automatically", "Delete the original after preparing"]) {
      expect(view.queryByLabelText(label)).toBeNull();
    }
    expect(view.queryByText(/changes files without asking/i)).toBeNull();
  });

  it("keeps the audit one short button away, named for the setting", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse() : response(audit));
    const view = render(() => <SettingsPage api={api} />);
    await view.findByLabelText("Scan interval (seconds)");

    const button = view.getByRole("button", { name: "Show history of Parallel conversions" });

    expect(button).toHaveTextContent("History");
  });

  it("orders settings inside a group the way the group lists them, not the way the server sent them", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse(1, [
      { key: "max_probe_workers", value: 2, revision: 1 },
      { key: "settle_seconds", value: 120, revision: 1 },
      { key: "scan_interval_seconds", value: 60, revision: 1 },
    ]) : response([]));
    const view = render(() => <SettingsPage api={api} />);
    await view.findByLabelText("Scan interval (seconds)");

    const labels = Array.from(within(view.getByRole("region", { name: "Library scan" })).getAllByRole("spinbutton")).map((input) => input.getAttribute("aria-label"));

    expect(labels).toEqual(["Scan interval (seconds)", "Wait after last change (seconds)", "Parallel checks"]);
  });
});


describe("settings: keyboard", () => {
  it("lists the shortcuts, so they can be read without hunting for the key that opens them", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse() : response([]));
    const view = render(() => <SettingsPage api={api} />);

    const section = await view.findByRole("region", { name: "Keyboard shortcuts" });

    expect(within(section).getByText("Add the file to the selection or take it out")).toBeInTheDocument();
  });

  it("says what a setting does, its limits and whether it was changed, without revision jargon", async () => {
    const api = apiFor((path) => path === "/settings" ? settingsResponse(7, [
      { key: "settle_seconds", value: 120, revision: 7, default_value: 60, source: "db_override", last_changed_revision: 5, effective_revision: 7 } as Setting,
    ]) : response([]));
    const view = render(() => <SettingsPage api={api} />);
    const wait = (await view.findByLabelText("Wait after last change (seconds)")).closest(".setting") as HTMLElement;

    expect(wait).toHaveTextContent(/still being copied or imported/);
    expect(wait).toHaveTextContent("Allowed: 0 to 86400. Default: 60. Changed from the default.");
    expect(wait).not.toHaveTextContent(/revision|source/i);
  });
});

import { For, Show, createMemo, createSignal, onMount, type Component } from "solid-js";
import { SessionError } from "../../lib/api";
import { Button } from "../../ui";
import { N, t } from "../../lib/i18n";
import { ShortcutsPanel } from "../help/ShortcutsPanel";

type SettingsApi = { request: (path: string, init?: RequestInit) => Promise<Response> };
type SettingValue = number | boolean | string[];
type Setting = {
  key: string;
  value: unknown;
  revision: number;
  default_value?: unknown;
  source?: "default" | "db_override" | "config";
  last_changed_revision?: number | null;
  effective_revision?: number;
  read_only?: boolean;
  restart_required?: boolean;
  guidance?: string;
};
type AuditEntry = { key: string; revision: number; actor: string; old_value: unknown; new_value: unknown };
type SettingsEnvelope = { revision: number; settings: Setting[] };
type Definition = { label: string; hint?: string; kind: "number" | "boolean" | "lines"; min?: number; max?: number };

const DEFINITIONS: Record<string, Definition> = {
  scan_interval_seconds: { label: N("Scan interval (seconds)"), hint: N("How often the libraries are scanned for new and changed files."), kind: "number", min: 5, max: 86400 },
  settle_seconds: { label: N("Wait after last change (seconds)"), hint: N("A new or changed file is left alone for this long, so files still being copied or imported are not touched."), kind: "number", min: 0, max: 86400 },
  scan_exclude_dirs: { label: N("Folders the scan skips"), kind: "lines" },
  max_ffmpeg_workers: { label: N("Parallel preparations"), hint: N("How many files are prepared at once. More is faster but loads the server. Verify jobs are not counted."), kind: "number", min: 1, max: 32 },
  max_probe_workers: { label: N("Parallel checks"), hint: N("How many files are read at once by the scan and by Verify jobs. Preparing has its own limit."), kind: "number", min: 1, max: 32 },
  event_retention_days: { label: N("Keep events for (days)"), hint: N("How long events in the activity feed are kept."), kind: "number", min: 1, max: 365 },
  log_retention_days: { label: N("Keep logs for (days)"), hint: N("How long job logs are kept."), kind: "number", min: 1, max: 365 },
  job_retention_days: { label: N("Keep jobs for (days)"), hint: N("How long finished jobs are kept."), kind: "number", min: 1, max: 365 },
  target_crf: { label: N("CRF"), hint: N("Quality of a re-encode. Lower values keep more detail and need more space."), kind: "number", min: 0, max: 51 },
  target_prefer_hevc10: { label: N("Prefer HEVC 10-bit"), hint: N("Re-encodes H.264 to HEVC 10-bit even for SDR and 1080p video."), kind: "boolean" },
  target_keep_av1: { label: N("Keep AV1"), hint: N("Copies AV1 video instead of re-encoding it. Needs an Apple TV 4K of the 3rd generation."), kind: "boolean" },
  target_fix_dts: { label: N("Fix negative start times (DTS)"), hint: N("Corrects a negative first video timestamp, for files whose playback stumbles on it."), kind: "boolean" },
};

/** How settings are grouped on the page; anything not listed lands in "Other", read-only. */
const GROUPS: ReadonlyArray<{ id: string; title: string; keys: readonly string[] }> = [
  { id: "scanning", title: N("Library scan"), keys: ["scan_interval_seconds", "settle_seconds", "max_probe_workers", "scan_exclude_dirs"] },
  { id: "target", title: N("Target profile"), keys: ["target_crf", "target_prefer_hevc10", "target_keep_av1", "target_fix_dts"] },
  { id: "automation", title: N("Automation"), keys: ["max_ffmpeg_workers"] },
  { id: "retention", title: N("Retention"), keys: ["event_retention_days", "log_retention_days", "job_retention_days"] },
];

const labelOf = (definition: Definition | undefined) => (definition ? t(definition.label) : undefined);
/** Settings shown without a control, with the sentence that says what they do. */
const SHOWN_ONLY: Record<string, { label: string; hint: string }> = {
  delete_original: { label: N("Delete original"), hint: N("Removes the old non-MP4 container once its new MP4 passes verification. It does not decide whether a file may be encoded.") },
};

const labelFor = (key: string) => labelOf(DEFINITIONS[key]) ?? (SHOWN_ONLY[key] ? t(SHOWN_ONLY[key].label) : undefined) ?? key.replaceAll("_", " ").replace(/^\w/, (character) => character.toUpperCase());
const safeValue = (value: unknown) => typeof value === "string" ? value : JSON.stringify(value);
const linesOf = (value: unknown): string[] => (Array.isArray(value) ? value.map(String) : []);
/** What is worth knowing about a value besides what it does: its limits, its default and whether it was changed. */
function facts(setting: Setting, definition: Definition | undefined): string {
  const parts: string[] = [];
  if (definition?.kind === "number") parts.push(t("Allowed: {min} to {max}.", { min: definition.min ?? 0, max: definition.max ?? 0 }));
  if (definition?.kind === "boolean" && typeof setting.default_value === "boolean") parts.push(setting.default_value ? t("Default: on.") : t("Default: off."));
  else if (definition?.kind === "number" && setting.default_value !== undefined && setting.default_value !== null) parts.push(t("Default: {value}.", { value: String(setting.default_value) }));
  if (setting.source === "db_override") parts.push(t("Changed from the default."));
  return parts.join(" ");
}
const idempotencyKey = () => `settings-${crypto.randomUUID()}`;

async function readEnvelope(response: Response): Promise<SettingsEnvelope> {
  if (!response.ok) throw new Error("settings unavailable");
  const body = (await response.json()) as { data?: SettingsEnvelope };
  if (!body.data || typeof body.data.revision !== "number" || !Array.isArray(body.data.settings)) throw new Error("settings unavailable");
  return body.data;
}

async function readAudit(response: Response): Promise<AuditEntry[]> {
  if (!response.ok) throw new Error("audit unavailable");
  const body = (await response.json()) as { data?: AuditEntry[] };
  return Array.isArray(body.data) ? body.data : [];
}

export const SettingsPage: Component<{ api: SettingsApi }> = (props) => {
  const [settings, setSettings] = createSignal<Setting[]>([]);
  const [revision, setRevision] = createSignal<number | null>(null);
  const [draft, setDraft] = createSignal<Record<string, SettingValue>>({});
  const [audit, setAudit] = createSignal<AuditEntry[]>([]);
  const [auditKey, setAuditKey] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [sessionExpired, setSessionExpired] = createSignal(false);
  const [conflict, setConflict] = createSignal(false);
  const [savedMessage, setSavedMessage] = createSignal<string | null>(null);

  const hasDraft = () => Object.keys(draft()).length > 0;
  const validationErrors = () => Object.entries(draft()).flatMap(([key, value]) => {
    const definition = DEFINITIONS[key];
    const numericValue = typeof value === "number" ? value : Number.NaN;
    return definition?.kind === "number" && (!Number.isInteger(numericValue) || numericValue < (definition.min ?? 0) || numericValue > (definition.max ?? Number.MAX_SAFE_INTEGER))
      ? [t("{label} must be between {min} and {max}.", { label: t(definition.label), min: definition.min ?? 0, max: definition.max ?? 0 })]
      : [];
  });

  const applyEnvelope = (envelope: SettingsEnvelope, preserveDraft = false, merge = false) => {
    if (merge) {
      const byKey = new Map(settings().map((setting) => [setting.key, setting]));
      for (const setting of envelope.settings) byKey.set(setting.key, setting);
      setSettings([...byKey.values()]);
    } else {
      setSettings(envelope.settings);
    }
    setRevision(envelope.revision);
    if (!preserveDraft) setDraft({});
  };

  const loadAudit = async (key: string) => {
    try {
      setAudit(await readAudit(await props.api.request(`/settings/audit?key=${encodeURIComponent(key)}`)));
    } catch (caught) {
      if (caught instanceof SessionError) setSessionExpired(true);
      else setError(t("Unable to load the change history."));
    }
  };

  const load = async (preserveDraft = false) => {
    setLoading(true);
    setError(null);
    setSavedMessage(null);
    try {
      const envelope = await readEnvelope(await props.api.request("/settings"));
      applyEnvelope(envelope, preserveDraft);
      const firstKey = envelope.settings.find((setting) => DEFINITIONS[setting.key])?.key;
      if (firstKey) {
        setAuditKey(firstKey);
        await loadAudit(firstKey);
      }
    } catch (caught) {
      if (caught instanceof SessionError) setSessionExpired(true);
      else setError(t("Unable to load runtime settings."));
    } finally {
      setLoading(false);
    }
  };

  const change = (key: string, raw: string | boolean) => {
    const value = typeof raw === "boolean" ? raw
      : DEFINITIONS[key]?.kind === "lines" ? raw.split("\n")
      : raw === "" ? Number.NaN : Number(raw);
    setDraft((current) => ({ ...current, [key]: value }));
    setSavedMessage(null);
  };

  const save = async () => {
    if (revision() === null || validationErrors().length > 0 || !hasDraft() || saving()) return;
    setSaving(true);
    setError(null);
    setConflict(false);
    try {
      const response = await props.api.request("/settings", {
        method: "PATCH",
        headers: { "Content-Type": "application/json", "Idempotency-Key": idempotencyKey() },
        body: JSON.stringify({ settings: draft(), expected_revision: revision() }),
      });
      if (response.status === 409) {
        setConflict(true);
        await load(true);
        return;
      }
      if (response.status === 422 && draft().scan_exclude_dirs !== undefined) {
        setError(t("A setting was refused. Folder entries must be plain names or paths below a library: no leading /, no .., at most 100."));
        return;
      }
      const envelope = await readEnvelope(response);
      applyEnvelope(envelope, false, true);
      setSavedMessage(t("Settings saved."));
    } catch (caught) {
      if (caught instanceof SessionError) setSessionExpired(true);
      else setError(t("Unable to save settings."));
    } finally {
      setSaving(false);
    }
  };

  const discard = () => {
    setDraft({});
    setConflict(false);
    setError(null);
  };

  onMount(() => void load());

  const groups = createMemo(() => {
    const claimed = new Set(GROUPS.flatMap((group) => group.keys));
    const listed = GROUPS.map((group) => ({
      ...group,
      items: settings()
        .filter((setting) => group.keys.includes(setting.key))
        .sort((a, b) => group.keys.indexOf(a.key) - group.keys.indexOf(b.key)),
    }));
    const other = { id: "other", title: N("Other"), keys: [] as readonly string[], items: settings().filter((setting) => !claimed.has(setting.key)) };
    return [...listed, other].filter((group) => group.items.length > 0);
  });

  const renderSetting = (setting: Setting) => {
    const definition = DEFINITIONS[setting.key];
    const currentValue = () => draft()[setting.key] ?? setting.value;
    const inputId = `setting-${setting.key}`;
    return (
      <article class="setting">
        <Show
          when={definition && !setting.read_only}
          fallback={
            <div>
              <label for={inputId}>{labelFor(setting.key)}</label>
              <input id={inputId} class="input" aria-label={labelFor(setting.key)} value={safeValue(setting.value)} disabled />
              <Show when={SHOWN_ONLY[setting.key]}>{(shown) => <small class="help">{t(shown().hint)}</small>}</Show>
              <small class="help">{setting.guidance ?? t("Set in the configuration file. Change it there and restart the manager.")}</small>
            </div>
          }
        >
          <label for={inputId}>{labelOf(definition)}</label>
          <Show when={definition?.kind === "lines"}>
            <textarea
              id={inputId}
              class="input lines"
              aria-label={labelOf(definition)}
              aria-describedby={`${inputId}-hint`}
              rows={Math.min(8, Math.max(3, linesOf(currentValue()).length + 1))}
              spellcheck={false}
              value={linesOf(currentValue()).join("\n")}
              onInput={(event) => change(setting.key, event.currentTarget.value)}
            />
            <small class="help" id={`${inputId}-hint`}>{t("One folder per line. A name such as Extras skips every folder of that name in every library; a path such as Film/Trailer skips only that folder. Files already listed from a skipped folder show as missing after the next scan.")}</small>
          </Show>
          <Show
            when={definition?.kind === "boolean"}
            fallback={<Show when={definition?.kind !== "lines"}><input id={inputId} class="input" aria-label={labelOf(definition)} type="number" min={definition?.min} max={definition?.max} step="1" value={String(currentValue())} onInput={(event) => change(setting.key, event.currentTarget.value)} /></Show>}
          >
            <input id={inputId} aria-label={labelOf(definition)} type="checkbox" checked={Boolean(currentValue())} onChange={(event) => change(setting.key, event.currentTarget.checked)} />
          </Show>
          <Show when={definition?.hint}>
            <small class="help">{t(definition?.hint ?? "")}</small>
          </Show>
          <small class="help">{facts(setting, definition)}</small>
          <Show when={draft()[setting.key] !== undefined && validationErrors().some((message) => message.startsWith(labelOf(definition) ?? ""))}>
            <p class="notice bad" role="alert">{t("{label} must be between {min} and {max}.", { label: labelOf(definition) ?? "", min: definition?.min ?? 0, max: definition?.max ?? 0 })}</p>
          </Show>
        </Show>
        <Button variant="quiet" aria-label={t("Show history of {label}", { label: labelFor(setting.key) })} onClick={() => { setAuditKey(setting.key); void loadAudit(setting.key); }}>{t("History")}</Button>
        <Show when={auditKey() === setting.key}>
          <section class="audit" role="region" aria-label={t("{label} history", { label: labelFor(setting.key) })}>
            <Show when={audit().length > 0} fallback={<p class="muted">{t("No changes recorded yet.")}</p>}>
              <ul class="plain"><For each={audit()}>{(entry) => <li>{entry.actor}: {safeValue(entry.old_value)} → {safeValue(entry.new_value)} ({t("revision {revision}", { revision: entry.revision })})</li>}</For></ul>
            </Show>
          </section>
        </Show>
      </article>
    );
  };

  return (
    <section class="settings" aria-labelledby="settings-title">
      <header class="settings-head">
        <h2 id="settings-title">{t("Runtime settings")}</h2>
        <span class="muted">{t("Revision {revision}", { revision: String(revision() ?? "—") })}</span>
      </header>
      <Show when={loading()}><p class="muted">{t("Loading settings")}</p></Show>
      <Show when={sessionExpired()}><div class="notice bad" role="alert">{t("Session expired. Sign in again to view runtime settings.")}</div></Show>
      <Show when={!loading() && !sessionExpired()}>
        <Show when={error()}><div class="notice bad" role="alert">{error()}</div></Show>
        <Show when={conflict()}>
          <div class="notice bad" role="alert">{t("Someone changed these settings first. The current values were loaded; your edits are still here.")}</div>
          <div class="settings-actions">
            <Button onClick={() => { discard(); void load(); }}>{t("Use the current values")}</Button>
            <Button variant="quiet" onClick={discard}>{t("Discard my edits")}</Button>
          </div>
        </Show>
        <For each={groups()}>{(group) => (
          <section class="setting-group" role="region" aria-label={t(group.title)}>
            <h3>{t(group.title)}</h3>
            <Show when={group.id === "other"}>
              <p class="muted">{t("Read-only. These come from the configuration file; change them there and restart the manager.")}</p>
            </Show>
            <Show when={group.id === "target"}>
              <p class="muted">{t("Verify and Prepare in the manager calculate against these values. Changing a value sets the encoding need of every file to unknown and checks the files again. Jobs already queued have to be queued again. The command line and the import hook do not read it.")}</p>
            </Show>
            <For each={group.items}>{renderSetting}</For>
          </section>
        )}</For>
        <section class="setting-group" role="region" aria-label={t("Keyboard shortcuts")}>
          <h3>{t("Keyboard shortcuts")}</h3>
          <ShortcutsPanel />
        </section>
        <Show when={validationErrors().length > 0}><div class="notice bad" role="alert">{validationErrors()[0]}</div></Show>
        <div class="savebar">
          <Show when={hasDraft()}><p role="status">{t("Unsaved changes")}</p></Show>
          <Show when={savedMessage()}><p role="status">{savedMessage()}</p></Show>
          <Button variant="primary" disabled={!hasDraft() || validationErrors().length > 0 || saving()} onClick={() => void save()}>{t("Save settings")}</Button>
        </div>
      </Show>
    </section>
  );
};

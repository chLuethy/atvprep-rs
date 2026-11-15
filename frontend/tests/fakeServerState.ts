/**
 * What the real server adds to every file row: its `state` and `preparable` (Dateizustand). The tests'
 * fake servers call this on their responses, so a fixture only names the stored verdict. The app never
 * derives these itself; this stands in for the server's decision.
 */
export function withServerState<T>(data: T): T {
  if (Array.isArray(data)) return data.map(withServerState) as T;
  if (typeof data !== "object" || data === null) return data;
  const row = Object.fromEntries(Object.entries(data).map(([key, value]) => [key, withServerState(value)])) as Record<string, unknown>;
  if (typeof row.compliance_state === "string" && typeof row.discovery_state === "string" && row.state === undefined) {
    const open = row.discovery_state === "present" && Number(row.pending_sidecars ?? 0) > 0;
    const state = row.discovery_state === "missing" ? "missing" : row.compliance_state === "clean" && open ? "embed_open" : row.compliance_state;
    row.state = state;
    row.preparable = row.discovery_state === "present" && (["warning", "blocker", "embed_open"].includes(String(state)) || row.desired_action === "fix" || open);
  }
  return row as T;
}

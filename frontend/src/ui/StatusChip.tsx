import type { ParentComponent } from "solid-js";
import { Icon, type IconName } from "./icons";

export type ChipKind = "ok" | "warn" | "bad" | "neutral";

/**
 * State is words plus a symbol plus a tint; the tint is never the only signal. A `live` chip is one
 * whose work is going on right now: it breathes in a loop for as long as it says so.
 */
export const StatusChip: ParentComponent<{ kind: ChipKind; icon: IconName; live?: boolean }> = (props) => (
  <span
    class="chip"
    classList={{ ok: props.kind === "ok", warn: props.kind === "warn", bad: props.kind === "bad", neutral: props.kind === "neutral", live: !!props.live }}
  >
    <Icon name={props.icon} size={12} />
    <span class="chip-text">{props.children}</span>
  </span>
);

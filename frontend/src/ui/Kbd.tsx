import type { ParentComponent } from "solid-js";

/** A shortcut hint. Assistive tech skips it: the control it sits in already has a name. */
export const Kbd: ParentComponent = (props) => <kbd class="kbd" aria-hidden="true">{props.children}</kbd>;

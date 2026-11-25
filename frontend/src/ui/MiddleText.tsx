import type { Component } from "solid-js";
import { splitMiddle } from "../lib/mediaName";

/**
 * One-line text that is cut in the middle when the space runs out: the head
 * gives way with an ellipsis, the last characters stay. The full name is the
 * title of the element; the text content stays the whole text for copying.
 */
export const MiddleText: Component<{ text: string; full?: string; tail?: number; class?: string }> = (props) => {
  const parts = () => splitMiddle(props.text, props.tail ?? 12);
  return (
    <span class={["mid", props.class]} title={props.full ?? props.text}>
      <span class="mid-head">{parts().head}</span>
      <span class="mid-tail">{parts().tail}</span>
    </span>
  );
};

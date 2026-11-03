import { createEffect, onMount, type Component } from "solid-js";

/** Where each bar last stood, so a bar that is built anew carries on from there instead of from the start. */
const shown = new Map<string, number>();
const REMEMBERED = 200;

/**
 * A thin determinate bar. The fill grows by transform, which glides between two reads of the server
 * without touching layout; the value and the name are on the bar for assistive tech. Give bars of
 * the same thing the same `track`: a row that is rebuilt on every read then still moves smoothly.
 */
export const ProgressBar: Component<{ value: number; label: string; track?: string }> = (props) => {
  const percent = () => Math.max(0, Math.min(100, props.value));
  let fill: HTMLSpanElement | undefined;
  const place = (value: number) => { if (fill) fill.style.transform = `scaleX(${value / 100})`; };

  onMount(() => {
    const from = props.track ? shown.get(props.track) : undefined;
    if (from === undefined || from === percent() || !fill) return;
    place(from);
    void fill.offsetWidth; // commit the starting point, so the next change is a transition
  });
  createEffect(() => {
    const value = percent();
    place(value);
    if (!props.track) return;
    shown.delete(props.track);
    shown.set(props.track, value);
    if (shown.size > REMEMBERED) shown.delete(shown.keys().next().value as string);
  });

  return (
    <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={percent()} aria-label={props.label}>
      <span ref={fill} class="bar-fill" style={`transform: scaleX(${percent() / 100})`} />
    </div>
  );
};

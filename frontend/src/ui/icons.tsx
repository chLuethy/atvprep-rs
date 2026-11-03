import type { Component } from "solid-js";

const PATHS = {
  library: "M2.5 3.5h11v9h-11zM6 3.5v9M2.5 7h3.5M10 3.5v9",
  overview: "M2.5 13V8.5M6.5 13V3.5M10.5 13V6.5M14 13H2",
  activity: "M1.5 8h3l2-5 3 10 2-5h3",
  settings: "M3 5h10M3 11h10M6 3.5v3M10 9.5v3",
  search: "M7 2.8a4.2 4.2 0 1 0 0 8.4 4.2 4.2 0 0 0 0-8.4zM10.2 10.2L14 14",
  check: "M3 8.5l3.2 3L13 4.8",
  tag: "M8 2.8a5.2 5.2 0 1 0 0 10.4 5.2 5.2 0 0 0 0-10.4zM8 2.8v10.4",
  x: "M4 4l8 8M12 4l-8 8",
  dot: "M8 3.5a4.5 4.5 0 1 0 0 9 4.5 4.5 0 0 0 0-9z",
  warn: "M8 2.5l6 10.5H2zM8 7v3M8 11.6v.4",
  transcode: "M3 5.5h9l-2.5-2.5M13 10.5H4l2.5 2.5",
  back: "M10 3.5L5.5 8l4.5 4.5",
  lock: "M4.5 7.5h7v5.5h-7zM6 7.5V5.5a2 2 0 0 1 4 0v2",
  menu: "M2.5 4.5h11M2.5 8h11M2.5 11.5h11",
  clock: "M8 2.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11zM8 5v3.2l2.2 1.3",
  folder: "M2 4.5a1 1 0 0 1 1-1h3.2l1.6 1.8H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z",
  file: "M4 2.5h5l3 3v8H4zM9 2.5v3h3",
  chevron: "M6 3.5L10.5 8 6 12.5",
  columns: "M2.5 3.5h11v9h-11zM6.2 3.5v9M9.8 3.5v9",
  rows: "M2.5 3.5h11v9h-11zM2.5 7h11M2.5 10h11",
  keyboard: "M1.5 4.5h13v7h-13zM4 7h.5M7.75 7h.5M11.5 7h.5M5 9.5h6",
} as const;

export type IconName = keyof typeof PATHS;

/** Stroke icon on a 16px grid. Decorative: the label next to it carries the meaning. */
export const Icon: Component<{ name: IconName; size?: number }> = (props) => (
  <svg
    width={props.size ?? 14}
    height={props.size ?? 14}
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d={PATHS[props.name]} />
  </svg>
);

import { createSignal, untrack, type Accessor } from "solid-js";

/** Rows kept rendered above and below what is on screen. */
const OVERSCAN = 20;
/** The window moves in steps of this many rows, so it is not rebuilt on every pixel of scrolling. */
const SNAP = 10;
/** At least this many rows count as on screen, for a scroller that has no height yet. */
const MIN_VISIBLE = 30;
/** The height of a row until one has been measured. */
const ESTIMATE = 44;

export type RowRange = { start: number; end: number };

/**
 * Which rows of a long list to render. The list scrolls inside `scroller`; only the rows near the visible part are
 * built, and two spacers of `rowHeight` per skipped row keep the scroll bar and the position true. The row height is
 * the average of the first rows rendered, so rows of unequal height drift a little but never change what is listed.
 */
export function createRowWindow(options: {
  total: Accessor<number>;
  scroller: () => HTMLElement | null | undefined;
  /** How far the first row sits below the top of the scrolled content (a heading row, a note). */
  origin: () => number;
  /** The rows rendered right now, to measure their height. */
  rendered: () => Element[];
  estimate?: number;
}) {
  const [range, setRange] = createSignal<RowRange>({ start: 0, end: MIN_VISIBLE + OVERSCAN }, { equals: (a, b) => a.start === b.start && a.end === b.end });
  const [height, setHeight] = createSignal(options.estimate ?? ESTIMATE);
  let measured = false;
  const down = (index: number) => Math.max(0, Math.floor(index / SNAP) * SNAP);
  const up = (index: number) => Math.ceil(index / SNAP) * SNAP;

  const recompute = () => {
    if (!measured) {
      const rows = options.rendered();
      const average = rows.length > 0 ? rows.reduce((sum, row) => sum + row.getBoundingClientRect().height, 0) / rows.length : 0;
      if (average > 0) { measured = true; setHeight(average); }
    }
    const scroller = options.scroller();
    const rowHeight = untrack(height);
    const first = scroller ? Math.floor(Math.max(0, scroller.scrollTop - options.origin()) / rowHeight) : 0;
    const visible = Math.max(MIN_VISIBLE, Math.ceil((scroller?.clientHeight ?? 0) / rowHeight) + 1);
    setRange({ start: down(first - OVERSCAN), end: up(first + visible + OVERSCAN) });
  };

  /** Make sure the row at `index` is rendered, e.g. before it gets the focus. */
  const reveal = (index: number) => {
    const { start, end } = untrack(range);
    if (index >= start && index < end) return;
    setRange({ start: down(index - OVERSCAN), end: up(index + MIN_VISIBLE) });
  };

  /** Follow the scrolling of the scroller; returns the cleanup. */
  const attach = () => {
    const scroller = options.scroller();
    recompute();
    if (!scroller) return undefined;
    scroller.addEventListener("scroll", recompute, { passive: true });
    return () => scroller.removeEventListener("scroll", recompute);
  };

  /** The range cut to the list as it is now. */
  const shown = (): RowRange => {
    const total = options.total();
    const { start, end } = range();
    return { start: Math.min(start, Math.max(0, total - MIN_VISIBLE)), end: Math.min(end, total) };
  };
  return { shown, rowHeight: height, recompute, reveal, attach };
}

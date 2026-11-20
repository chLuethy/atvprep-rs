/** Width up to which a page with two columns is a single column and its height just follows the content. */
/* The one source of this breakpoint in code. CSS cannot share a constant, so every `@media (max-width: 1180px)`
   in src/styles repeats the literal; tests/theme-contract.test.ts keeps them equal to this number. */
export const SINGLE_COLUMN_MAX = 1180;
/** The least height of a work area, so a short window scrolls the page instead of squeezing the list to nothing. */
const MIN_HEIGHT = 420;

/**
 * Give `split` the height that makes its bottom edge line up with the bottom edge of `anchor`
 * (the sidebar's appearance buttons), so list, selection bar and inspector end on one line.
 * It measures again whenever the page, the block above the split or the anchor changes size,
 * and stops measuring (returns the cleanup) when the page goes away. In a single column the
 * height is left to the content.
 */
export function fitBottomTo(options: {
  page: () => HTMLElement | undefined;
  split: () => HTMLElement | undefined;
  anchor: () => HTMLElement | null;
}): () => void {
  const apply = () => {
    const page = options.page();
    const split = options.split();
    if (!page || !split) return;
    const anchor = options.anchor();
    if (!anchor || window.innerWidth <= SINGLE_COLUMN_MAX) {
      split.style.removeProperty("height");
      return;
    }
    // The top of the split with the page scrolled to its start: the page may be scrolled right now.
    const top = split.getBoundingClientRect().top + page.scrollTop;
    split.style.height = `${Math.max(MIN_HEIGHT, anchor.getBoundingClientRect().bottom - top)}px`;
  };
  apply();
  const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(apply);
  const above = options.split()?.previousElementSibling;
  for (const element of [options.page(), above]) if (element) observer?.observe(element);
  // The anchor moves when what sits below it changes size, so every container around it is watched too.
  for (let element = options.anchor(); element && element !== document.body; element = element.parentElement) observer?.observe(element);
  window.addEventListener("resize", apply);
  return () => {
    observer?.disconnect();
    window.removeEventListener("resize", apply);
  };
}

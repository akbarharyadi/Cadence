/**
 * The rows a long list builds (TrackList): the ones on screen plus a margin, as a range
 * `[first, last)` of row positions. Plain numbers in, so a test can scroll it without a page.
 *
 * The range moves in steps, not row by row: while the rows on screen (plus a small `keep` margin)
 * are still inside the built range, it stays as it is. Scrolling then changes no rows and no
 * padding for most frames; when it does move, it builds `margin` rows ahead again. Moving every
 * 54 px meant rebuilding rows and the list's padding (a layout of the whole list) on almost every
 * frame of a scroll.
 */

export interface Range {
  first: number;
  last: number;
}

export interface Viewport {
  /** Where the list's top is, relative to the scroller's visible top (negative once scrolled past). */
  top: number;
  /** The scroller's visible height. */
  height: number;
  /** Rows in the list. */
  count: number;
  /** Each row's height in px. */
  row: number;
}

/** Rows built beyond the screen on each side when the range moves. */
export const MARGIN = 12;
/** The range moves once fewer than this many built rows are left beyond the screen. */
export const KEEP = 3;

/** The rows on screen, `[from, to)`, clamped to the list. */
export function onScreen(v: Viewport): Range {
  const first = Math.max(0, Math.min(v.count, Math.floor(-v.top / v.row)));
  const last = Math.max(first, Math.min(v.count, Math.ceil((v.height - v.top) / v.row)));
  return { first, last };
}

/**
 * The range to build: `current` itself (the same object) while it still covers the screen with
 * `keep` rows to spare (or reaches the list's end), else a new range with `margin` rows each side.
 */
export function nextRange(current: Range, v: Viewport, margin = MARGIN, keep = KEEP): Range {
  const seen = onScreen(v);
  const needFirst = Math.max(0, seen.first - keep);
  const needLast = Math.min(v.count, seen.last + keep);
  const inside = current.first <= needFirst && current.last >= needLast && current.last <= v.count;
  if (inside) return current;
  const first = Math.max(0, seen.first - margin);
  const last = Math.max(first, Math.min(v.count, seen.last + margin));
  return { first, last };
}

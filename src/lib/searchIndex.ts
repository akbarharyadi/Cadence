// Matching for the Search page and the command palette: one place, one cached folded index.
import type { Album, Track } from "./api";

/** What songs, albums and artists are found by: the same for Search and the palette, so they share
 *  one index per list (it is kept per list, made with the fields first asked for). */
export const trackFields = (t: Track) => [t.title, t.subtitle ?? "", t.artist, t.album];
export const albumFields = (a: Album) => [a.title, a.artist];
export const artistFields = (a: { name: string }) => [a.name];

/** Case- and accent-insensitive: "pokemon" finds "Pokémon". */
export const fold = (s: string) => s.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase();

/**
 * A list's names, folded once per list (the library's lists are replaced whole when it changes, so
 * the list itself is the key), not on every key press. `items` is a plain copy of the list: the
 * store's lists are reactive proxies, and scanning tens of thousands of entries through one costs
 * far more than the matching itself. `all` is every field in one string (joined by a character no
 * typed text has), so "any field contains it" is one check per item.
 */
interface Index<T> {
  items: T[];
  rows: string[][];
  all: string[];
  fields: (x: T) => string[];
}

const SEP = "\u0001";
const cache = new WeakMap<object, Index<unknown>>();

function indexFor<T>(list: T[], fields: (x: T) => string[]): Index<T> {
  let ix = cache.get(list) as Index<T> | undefined;
  if (!ix) {
    ix = { items: [], rows: [], all: [], fields };
    cache.set(list, ix as Index<unknown>);
  }
  return ix;
}

/** Folds the next `n` items of the list (from where the last call stopped); true when it's all done. */
function step<T>(list: T[], ix: Index<T>, n: number): boolean {
  const len = list.length;
  const end = Math.min(len, ix.rows.length + n);
  for (let i = ix.rows.length; i < end; i++) {
    const x = list[i];
    const row = ix.fields(x).map(fold);
    ix.items.push(x);
    ix.rows.push(row);
    ix.all.push(row.join(SEP));
  }
  return ix.rows.length >= len;
}

function built<T>(list: T[], fields: (x: T) => string[]): Index<T> {
  const ix = indexFor(list, fields);
  if (ix.rows.length < list.length) step(list, ix, list.length);
  return ix;
}

/** The folded fields of every item (made the first time, then kept for this list). */
export function folded<T>(list: T[], fields: (x: T) => string[]): string[][] {
  return built(list, fields).rows;
}

/** Whether the list's index is made (the first search on it then costs only the matching). */
export function isFolded(list: object): boolean {
  const ix = cache.get(list);
  return !!ix && ix.rows.length >= (list as unknown[]).length;
}

type Idle = (cb: (d: { timeRemaining: () => number }) => void) => unknown;

/**
 * Makes the index ahead of the first search, a slice at a time while the window is idle (a
 * search box got focus, Search or the palette opened): the first key press then only matches.
 * Run outside any effect, so reading the store's lists here subscribes to nothing. A search that
 * comes before it's done finishes it at once.
 */
export function warm<T>(list: T[], fields: (x: T) => string[]): void {
  if (!list?.length || isFolded(list)) return;
  const ix = indexFor(list, fields);
  const idle = (globalThis as { requestIdleCallback?: Idle }).requestIdleCallback;
  const next = () => {
    if (idle) idle((d) => (step(list, ix, Math.max(200, Math.floor(d.timeRemaining() * 150))) ? undefined : next()));
    else setTimeout(() => (step(list, ix, 1500) ? undefined : next()), 0);
  };
  next();
}

/** Another way an item can match: its other names (an artist's spellings, a title's reading). */
export type Extra<T> = ((x: T) => boolean) | null | undefined;

/**
 * The first `max` items (in list order) with a field containing the folded text `q`; then, if
 * there's room, those found only by `extra` (so what the names match always comes first).
 */
export function matching<T>(list: T[], fields: (x: T) => string[], q: string, max: number, extra?: Extra<T>): T[] {
  const named = matchingNames(list, fields, q, max);
  if (!extra || named.length >= max) return named;
  return [...named, ...matchingOthers(list, fields, q, max - named.length, extra)];
}

/** `matching` without `extra`: the items whose names contain `q`. */
export function matchingNames<T>(list: T[], fields: (x: T) => string[], q: string, max: number): T[] {
  const { items, all } = built(list, fields);
  const out: T[] = [];
  for (let i = 0; i < all.length && out.length < max; i++) if (all[i].includes(q)) out.push(items[i]);
  return out;
}

/** The first `max` items found only by `extra` (their names don't contain `q`). */
export function matchingOthers<T>(list: T[], fields: (x: T) => string[], q: string, max: number, extra: Extra<T>): T[] {
  if (!extra || max <= 0) return [];
  const { items, all } = built(list, fields);
  const out: T[] = [];
  for (let i = 0; i < all.length && out.length < max; i++) if (!all[i].includes(q) && extra(items[i])) out.push(items[i]);
  return out;
}

/**
 * The best `max` matches: those whose first field (the title or name) starts with `q`, then those
 * that contain it there, then matches in the other fields. List order breaks ties. One pass over
 * the folded list, so it stays cheap on a library of tens of thousands of songs.
 */
export function ranked<T>(list: T[], fields: (x: T) => string[], q: string, max: number, extra?: Extra<T>): T[] {
  const { items, rows, all } = built(list, fields);
  const starts: T[] = [];
  const inTitle: T[] = [];
  const other: T[] = [];
  // Found only by another name or a reading: after everything the names match.
  const aliased: T[] = [];
  for (let i = 0; i < all.length; i++) {
    if (!all[i].includes(q)) {
      if (extra && aliased.length < max && extra(items[i])) aliased.push(items[i]);
      continue;
    }
    const first = rows[i][0] ?? "";
    if (first.startsWith(q)) {
      starts.push(items[i]);
      // Enough of the best kind: nothing later can rank higher.
      if (starts.length >= max) break;
    } else if (first.includes(q)) {
      if (inTitle.length < max) inTitle.push(items[i]);
    } else if (other.length < max) other.push(items[i]);
  }
  return [...starts, ...inTitle, ...other, ...aliased].slice(0, max);
}

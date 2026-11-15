// Matching for the Search page and the command palette: one place, one cached folded index.

/** Case- and accent-insensitive: "pokemon" finds "Pokémon". */
export const fold = (s: string) => s.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase();

// The library's names, folded once per library (the lists are replaced whole when it changes,
// so the list itself is the key), not on every key press.
const cache = new WeakMap<object, string[][]>();
export function folded<T>(list: T[], fields: (x: T) => string[]): string[][] {
  let f = cache.get(list);
  if (!f) {
    f = list.map((x) => fields(x).map(fold));
    cache.set(list, f);
  }
  return f;
}

/** Another way an item can match: its other names (an artist's spellings, a title's reading). */
export type Extra<T> = ((x: T) => boolean) | null | undefined;

/**
 * The first `max` items (in list order) with a field containing the folded text `q`; then, if
 * there's room, those found only by `extra` (so what the names match always comes first).
 */
export function matching<T>(list: T[], fields: (x: T) => string[], q: string, max: number, extra?: Extra<T>): T[] {
  const f = folded(list, fields);
  const out: T[] = [];
  for (let i = 0; i < list.length && out.length < max; i++) if (f[i].some((s) => s.includes(q))) out.push(list[i]);
  if (extra && out.length < max) {
    for (let i = 0; i < list.length && out.length < max; i++) if (!f[i].some((s) => s.includes(q)) && extra(list[i])) out.push(list[i]);
  }
  return out;
}

/**
 * The best `max` matches: those whose first field (the title or name) starts with `q`, then those
 * that contain it there, then matches in the other fields. List order breaks ties. One pass over
 * the folded list, so it stays cheap on a library of tens of thousands of songs.
 */
export function ranked<T>(list: T[], fields: (x: T) => string[], q: string, max: number, extra?: Extra<T>): T[] {
  const f = folded(list, fields);
  const starts: T[] = [];
  const inTitle: T[] = [];
  const other: T[] = [];
  // Found only by another name or a reading: after everything the names match.
  const aliased: T[] = [];
  for (let i = 0; i < list.length; i++) {
    const row = f[i];
    const first = row[0] ?? "";
    if (first.startsWith(q)) {
      starts.push(list[i]);
      // Enough of the best kind: nothing later can rank higher.
      if (starts.length >= max) break;
    } else if (inTitle.length < max && first.includes(q)) inTitle.push(list[i]);
    else if (row.some((s) => s.includes(q))) {
      if (other.length < max) other.push(list[i]);
    } else if (extra && aliased.length < max && extra(list[i])) aliased.push(list[i]);
  }
  return [...starts, ...inTitle, ...other, ...aliased].slice(0, max);
}

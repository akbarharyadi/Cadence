/** Home's picks, as plain functions (no Svelte), so a test can call them. */

/**
 * The album "Rediscover" shows on day `day`: one of `unplayed`, a different one each day, never
 * one of `skip` (the albums the cards before it show). Looks at a few albums, not the list.
 */
export function rediscover<T extends { id: string }>(unplayed: readonly T[], day: number, skip: (string | undefined)[]): T | undefined {
  const n = unplayed.length;
  for (let k = 0; k < Math.min(n, skip.length + 1); k++) {
    const a = unplayed[(((day + k) % n) + n) % n];
    if (a && !skip.includes(a.id)) return a;
  }
  return undefined;
}

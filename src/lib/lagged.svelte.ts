import { untrack } from "svelte";

/**
 * A value that follows `read()` at most once every `ms`: the first change at once, later ones
 * gathered and taken together when the gap is over (always ending on the latest). For page
 * sections worked out from something that changes with every song (the albums played lately):
 * with very short songs, or skipping through a shuffle, they'd be rebuilt and laid out every
 * second for nothing anyone reads. Call it while a component is being set up (it uses `$effect`).
 */
export function lagged<T>(read: () => T, ms: number): { readonly value: T } {
  let value = $state.raw(read());
  let last = -Infinity;
  $effect(() => {
    const next = read();
    if (next === untrack(() => value)) return;
    const take = () => {
      value = next;
      last = performance.now();
    };
    const wait = last + ms - performance.now();
    if (wait <= 0) {
      take();
      return;
    }
    const timer = setTimeout(take, wait);
    return () => clearTimeout(timer);
  });
  return {
    get value() {
      return value;
    },
  };
}

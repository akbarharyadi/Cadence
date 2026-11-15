/**
 * `run` at most once every `gap` ms however often it's asked for: the first ask runs at once,
 * asks during the gap become one more run when it's over (so the last change is never missed).
 * A run's failure is left to `run` (it should catch its own).
 */
export function throttled(run: () => unknown, gap: number): () => void {
  let last = -Infinity;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const go = () => {
    timer = undefined;
    last = Date.now();
    run();
  };
  return () => {
    if (timer) return;
    const wait = last + gap - Date.now();
    if (wait <= 0) go();
    else timer = setTimeout(go, wait);
  };
}

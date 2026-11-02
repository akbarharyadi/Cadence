/**
 * Equaliser bars that bounce while a song plays (`use:eqBars` on the element holding the bars).
 *
 * Their heights change eight times a second, without animating in between. A CSS animation makes
 * the page draw a frame 60 times a second for as long as something plays; on a phone that was
 * most of the app's processor time. Eight jumps a second read as a level meter and cost almost
 * nothing. One timer serves every set of bars on the page.
 */
const shown = new Set<HTMLElement>();
let timer: ReturnType<typeof setInterval> | undefined;
const reduced = typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

function tick() {
  if (document.hidden) return;
  const t = performance.now() / 1000;
  for (const el of shown) {
    const bars = el.children;
    for (let i = 0; i < bars.length; i++) {
      // Each bar bounces at its own pace, so the three never move together.
      const h = 0.25 + 0.75 * Math.abs(Math.sin(t * (2.6 + i * 1.45) + i * 1.9));
      (bars[i] as HTMLElement).style.transform = `scaleY(${h.toFixed(2)})`;
    }
  }
}

function start(el: HTMLElement) {
  if (reduced || shown.has(el)) return;
  shown.add(el);
  tick();
  timer ??= setInterval(tick, 125);
}

function stop(el: HTMLElement) {
  if (!shown.delete(el)) return;
  for (const bar of el.children) (bar as HTMLElement).style.transform = "";
  if (!shown.size) {
    clearInterval(timer);
    timer = undefined;
  }
}

/**
 * Called with each of the player's position updates (four a second), which redraw the page anyway:
 * the bars move then and once halfway to the next update, so they cost four frames a second
 * instead of eight.
 */
export function eqSync() {
  if (!shown.size) return;
  clearInterval(timer);
  tick();
  timer = setInterval(tick, 125);
}

/** Bounces the element's children while `on` (by default, while it's on the page). */
export function eqBars(el: HTMLElement, on = true) {
  if (on) start(el);
  return {
    update: (now: boolean) => (now ? start(el) : stop(el)),
    destroy: () => stop(el),
  };
}

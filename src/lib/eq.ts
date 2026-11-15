/**
 * Equaliser bars that bounce while a song plays (`use:eqBars` on the element holding the bars).
 *
 * Their heights change eight times a second, without animating in between. A CSS animation makes
 * the page draw a frame 60 times a second for as long as something plays; on a phone that was
 * most of the app's processor time. Eight jumps a second read as a level meter and cost almost
 * nothing. One timer serves every set of bars on the page, and it only runs while at least one set
 * is on screen (an IntersectionObserver: bars scrolled out of a long list, or under a closed
 * panel, cost nothing) and the page can be seen at all (hidden in the tray or minimized: no timer).
 */
import { onShowing, pageShowing } from "./showing.svelte.js";

/** Every set of bars on the page, and the ones of them on screen. */
const shown = new Set<HTMLElement>();
const onScreen = new Set<HTMLElement>();
let timer: ReturnType<typeof setTimeout> | undefined;
/** Between two heights; and how long after the halfway one to wait for the player's next update. */
const STEP = 125;
const LATE = 190;
/** The last tick came with a player update (the next one is the halfway one). */
let synced = false;
const reduced = typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

// Without IntersectionObserver (tests, old webviews) every set counts as on screen.
const observer =
  typeof IntersectionObserver !== "undefined"
    ? new IntersectionObserver((entries) => {
        for (const e of entries) {
          const el = e.target as HTMLElement;
          if (!shown.has(el)) continue;
          if (e.isIntersecting) onScreen.add(el);
          else onScreen.delete(el);
        }
        restart();
      })
    : undefined;

function tick() {
  if (!pageShowing()) return;
  const t = performance.now() / 1000;
  for (const el of onScreen) {
    const bars = el.children;
    for (let i = 0; i < bars.length; i++) {
      // Each bar bounces at its own pace, so the three never move together.
      const h = 0.25 + 0.75 * Math.abs(Math.sin(t * (2.6 + i * 1.45) + i * 1.9));
      (bars[i] as HTMLElement).style.transform = `scaleY(${h.toFixed(2)})`;
    }
  }
}

/** Whether the ticker should run now. */
function wanted() {
  return onScreen.size > 0 && pageShowing();
}

/**
 * The next tick, in `ms`. After a player update the next is the halfway one; after that it waits
 * for the next update (which draws a frame anyway) and only ticks by itself if that is late, so
 * the bars' eight heights a second share frames with the four updates: eight frames a second, not
 * the eleven a free-running timer drifting against the updates made.
 */
function schedule(ms: number) {
  clearTimeout(timer);
  timer = setTimeout(() => {
    timer = undefined;
    if (!wanted()) return;
    tick();
    const afterSync = synced;
    synced = false;
    schedule(afterSync ? LATE : STEP);
  }, ms);
}

/** Starts (from now, with one tick at once) or stops the ticker to match what's wanted. */
function restart() {
  clearTimeout(timer);
  timer = undefined;
  synced = false;
  if (!wanted()) return;
  tick();
  schedule(STEP);
}

// Hidden in the tray, minimized, another app on a phone: the ticker stops; back: it starts again.
onShowing(() => restart());

function start(el: HTMLElement) {
  if (reduced || shown.has(el)) return;
  shown.add(el);
  if (observer) observer.observe(el);
  else onScreen.add(el);
  if (!timer) restart();
}

function stop(el: HTMLElement) {
  if (!shown.delete(el)) return;
  observer?.unobserve(el);
  onScreen.delete(el);
  for (const bar of el.children) (bar as HTMLElement).style.transform = "";
  if (!onScreen.size) restart();
}

/**
 * Called with each of the player's position updates (four a second), which redraw the page anyway:
 * the bars move then and once halfway to the next update, so they cost four frames a second
 * instead of eight.
 */
export function eqSync() {
  if (!timer) return;
  tick();
  synced = true;
  schedule(STEP);
}

/** Bounces the element's children while `on` (by default, while it's on the page). */
export function eqBars(el: HTMLElement, on = true) {
  if (on) start(el);
  return {
    update: (now: boolean) => (now ? start(el) : stop(el)),
    destroy: () => stop(el),
  };
}

/** For tests: whether the ticker is running. */
export function eqTicking(): boolean {
  return timer !== undefined;
}

/**
 * Whether this page can be seen: one signal every timer in the app goes by (`app.showing` in
 * components, `pageShowing()` / `onShowing()` in plain modules such as eq.ts).
 *
 * It combines the page's own `document.visibilityState` with the app's word on its window
 * (`window-visible`, see winvis.rs): on Windows a window hidden in the tray or minimized only
 * reports "hidden" because Cadence tells WebView2 so, and on Linux and macOS a window hidden in the
 * tray may keep saying "visible". On Android the system WebView reports the page hidden itself
 * (the app never sends `window-visible` there).
 */

const docVisible = () => typeof document === "undefined" || document.visibilityState === "visible";

let windowVisible = true;
let lastShowing = docVisible();
const listeners = new Set<(showing: boolean) => void>();

class Showing {
  on = $state(lastShowing);
}
const signal = new Showing();

/** `html.page-hidden` while the page can't be seen: app.css pauses every CSS animation then (a
 *  window hidden in the tray on Linux or macOS may keep "visible" and keep drawing). */
function mark(showing: boolean) {
  if (typeof document !== "undefined") document.documentElement?.classList.toggle("page-hidden", !showing);
}

function update() {
  const now = windowVisible && docVisible();
  if (now === lastShowing) return;
  lastShowing = now;
  signal.on = now;
  mark(now);
  for (const f of [...listeners]) f(now);
}

if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", update);
  if (!lastShowing) mark(false);
}

/** True while the page can be seen. Reactive in components and effects. */
export function pageShowing(): boolean {
  return signal.on;
}

/** The app said this window was hidden (in the tray, minimized) or shown again. */
export function setWindowVisible(visible: boolean) {
  windowVisible = visible;
  update();
}

/** Calls `f` each time the page goes from showing to hidden or back. Returns the unsubscribe. */
export function onShowing(f: (showing: boolean) => void): () => void {
  listeners.add(f);
  return () => listeners.delete(f);
}

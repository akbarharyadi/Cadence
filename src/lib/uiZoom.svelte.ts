// Interface size (Settings > Appearance, Ctrl+= / Ctrl+- / Ctrl+0).
//
// Uses the webview's own zoom (`getCurrentWebview().setZoom`, the command `plugin:webview|set_webview_zoom`,
// permission `core:webview:allow-set-webview-zoom` in capabilities/desktop.json, main window only). That
// scales the page like browser zoom, so layout, `vh`, pointer coordinates and measured geometry all stay
// consistent; nothing in the page needs to know about it. The mini player is another window and is never
// scaled (this module is not loaded there). Always an absolute scale factor, never relative, so the
// webview's own zoom hotkeys (off: `zoomHotkeysEnabled` defaults to false in tauri.conf.json) could not
// drift it. If the call fails (permission missing, unsupported platform) the size falls back to 100%.

import { getCurrentWebview } from "@tauri-apps/api/webview";

export const ZOOM_STEPS = [0.9, 1, 1.1, 1.25, 1.5] as const;
export const ZOOM_KEY = "cadence-ui-zoom";

const onPhone = () => typeof navigator !== "undefined" && navigator.userAgent.includes("Android");

function nearest(v: number): number {
  let best: number = ZOOM_STEPS[1];
  for (const s of ZOOM_STEPS) if (Math.abs(s - v) < Math.abs(best - v)) best = s;
  return best;
}

/** The saved size (always one of the steps; 1 when nothing valid is saved or on a phone). */
export function savedZoom(): number {
  if (onPhone() || typeof localStorage === "undefined") return 1;
  try {
    const v = parseFloat(localStorage.getItem(ZOOM_KEY) ?? "");
    return Number.isFinite(v) ? nearest(v) : 1;
  } catch {
    return 1;
  }
}

let seq = 0;

/** Sets the webview's zoom. Resolves true when it took, false when it could not (never rejects). */
export async function applyZoom(z: number): Promise<boolean> {
  if (onPhone()) return false;
  try {
    await getCurrentWebview().setZoom(z);
    return true;
  } catch {
    return false;
  }
}

class UiZoom {
  value = $state(savedZoom());

  set(z: number) {
    if (onPhone()) return;
    const v = nearest(z);
    this.value = v;
    try {
      if (v === 1) localStorage.removeItem(ZOOM_KEY);
      else localStorage.setItem(ZOOM_KEY, String(v));
    } catch {
      // Private mode or a full disk: the size still applies for this session.
    }
    this.push(v);
  }

  private push(v: number) {
    const mine = ++seq;
    void applyZoom(v).then((ok) => {
      // Could not zoom: show (and stay at) 100%. The saved choice is kept for a build that can.
      if (!ok && mine === seq && this.value === v) this.value = 1;
    });
  }

  /** At startup: a saved size goes onto the webview (100% is the webview's own default). */
  start() {
    if (this.value !== 1) this.push(this.value);
  }

  /** One step larger (+1) or smaller (-1). */
  step(dir: 1 | -1) {
    const i = ZOOM_STEPS.indexOf(this.value as (typeof ZOOM_STEPS)[number]);
    const next = ZOOM_STEPS[Math.max(0, Math.min(ZOOM_STEPS.length - 1, (i < 0 ? 1 : i) + dir))];
    this.set(next);
  }

  reset() {
    this.set(1);
  }
}

export const uiZoom = new UiZoom();
// As early as the page can: this module is loaded with the page, ahead of any component.
uiZoom.start();

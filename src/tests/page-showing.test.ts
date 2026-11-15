// The window hidden in the tray or minimized: the page stops every timer of its own (and draws
// nothing), and when it's shown again it has the player's fresh status at once.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { emit } from "@tauri-apps/api/event";
import { app } from "#lib/store.svelte.js";
import { onShowing, pageShowing, setWindowVisible } from "#lib/showing.svelte.js";
import { eqBars, eqSync, eqTicking } from "#lib/eq.js";
import SleepLabel from "#lib/components/SleepLabel.svelte";
import { makeLibrary, makeStatus, mockBackend, resetApp, settle } from "./helpers";

let docState: DocumentVisibilityState = "visible";
function setDocument(state: DocumentVisibilityState) {
  docState = state;
  document.dispatchEvent(new Event("visibilitychange"));
}

beforeEach(() => {
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => docState });
  resetApp();
});

afterEach(() => {
  vi.useRealTimers();
  setWindowVisible(true);
  setDocument("visible");
  app.sleepEndsAt = null;
});

function bars() {
  const el = document.createElement("span");
  el.innerHTML = "<i></i><i></i><i></i>";
  document.body.append(el);
  return el;
}

describe("the page is showing", () => {
  it("only while both the document and the app's window are visible", () => {
    const seen: boolean[] = [];
    const off = onShowing((s) => seen.push(s));
    expect(pageShowing()).toBe(true);
    expect(app.showing).toBe(true);

    setWindowVisible(false);
    expect(app.showing).toBe(false);
    // The document turning hidden too changes nothing more.
    setDocument("hidden");
    expect(app.showing).toBe(false);
    setWindowVisible(true);
    expect(app.showing).toBe(false); // the document is still hidden
    setDocument("visible");
    expect(app.showing).toBe(true);

    // A document hidden by itself (Android, another app in front) counts as well.
    setDocument("hidden");
    expect(app.showing).toBe(false);
    setDocument("visible");
    off();
    expect(seen).toEqual([false, true, false, true]);
  });

  it("follows the app's window-visible event, and has the fresh status the moment it's shown", async () => {
    const { tracks, albums } = makeLibrary(3);
    let status = makeStatus({ queue: tracks.map((t) => t.id), index: 0, playing: true, position: 10, duration: 200 });
    mockBackend({
      get_library: () => ({ tracks, albums }),
      get_status: () => status,
    });
    await app.initMini();
    expect(app.status?.index).toBe(0);

    await emit("window-visible", false);
    await settle();
    expect(app.showing).toBe(false);

    // While hidden the song changed; the app sends one full status before saying "shown".
    status = makeStatus({ queue: tracks.map((t) => t.id), index: 2, playing: true, position: 3, duration: 200 });
    await emit("player-status", status);
    await emit("window-visible", true);
    await settle();
    expect(app.showing).toBe(true);
    expect(app.status?.index).toBe(2);
    expect(app.current?.id).toBe(tracks[2].id);
    expect(app.position).toBeCloseTo(3, 0);
  });
});

describe("timers stop while the page is hidden", () => {
  it("the equaliser bars' ticker", () => {
    vi.useFakeTimers();
    const el = bars();
    const action = eqBars(el);
    expect(eqTicking()).toBe(true);
    const bar = el.children[0] as HTMLElement;
    expect(bar.style.transform).toMatch(/scaleY/);

    setWindowVisible(false);
    expect(eqTicking()).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    bar.style.transform = "none";
    vi.advanceTimersByTime(5000);
    // The player's updates (the mini window, say) don't start it either.
    eqSync();
    vi.advanceTimersByTime(5000);
    expect(bar.style.transform).toBe("none");

    setWindowVisible(true);
    expect(eqTicking()).toBe(true);
    expect(bar.style.transform).toMatch(/scaleY/); // a height at once, not after a tick

    action.destroy();
    expect(eqTicking()).toBe(false);
    el.remove();
  });

  it("the bars share frames with the player's updates: one halfway tick, then wait for the next update", () => {
    vi.useFakeTimers();
    const el = bars();
    const action = eqBars(el);
    const bar = el.children[0] as HTMLElement;
    /** Whether the bars got new heights since the last look. */
    const moved = () => {
      const m = bar.style.transform !== "scale(7)";
      bar.style.transform = "scale(7)";
      return m;
    };
    moved();
    eqSync(); // an update
    expect(moved()).toBe(true);
    vi.advanceTimersByTime(125); // halfway
    expect(moved()).toBe(true);
    vi.advanceTimersByTime(125); // the next update is due: no tick of its own
    expect(moved()).toBe(false);
    eqSync();
    expect(moved()).toBe(true);
    // Updates stop coming: it carries on by itself, the first a little late, then every 125 ms.
    vi.advanceTimersByTime(125);
    expect(moved()).toBe(true);
    vi.advanceTimersByTime(189);
    expect(moved()).toBe(false);
    vi.advanceTimersByTime(1);
    expect(moved()).toBe(true);
    vi.advanceTimersByTime(125);
    expect(moved()).toBe(true);
    action.destroy();
    el.remove();
  });

  it("the sleep timer's countdown", async () => {
    vi.useFakeTimers();
    app.sleepEndsAt = Math.floor(Date.now() / 1000) + 125;
    const target = document.createElement("div");
    document.body.append(target);
    const c = mount(SleepLabel, { target });
    flushSync();
    expect(target.textContent).toBe("3 min");
    expect(vi.getTimerCount()).toBe(1);

    setWindowVisible(false);
    flushSync();
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(70_000);
    flushSync();
    expect(target.textContent).toBe("3 min"); // nothing counted while hidden

    setWindowVisible(true);
    flushSync();
    // Right at once on showing again (55 s left).
    expect(target.textContent).toBe("55 s");
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(1000);
    flushSync();
    expect(target.textContent).toBe("54 s");
    unmount(c);
    target.remove();
  });
});

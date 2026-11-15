import { fireEvent, render, screen } from "@testing-library/svelte";
import { emit } from "@tauri-apps/api/event";
import { afterAll, beforeAll, beforeEach, describe, expect, test } from "vitest";
import NowPlaying from "#lib/components/NowPlaying.svelte";
import { app } from "#lib/store.svelte.js";
import { barHeights, clearWaveforms, parseWaveform, waveMask } from "#lib/waveform.js";
import { makeLibrary, makeStatus, mockBackend, resetApp, settle } from "./helpers";

/** A waveform file as the backend stores it (src-tauri/src/waveform.rs, format 1). */
function waveBytes(n = 200, level = (i: number) => (i * 7) % 256): Uint8Array {
  const b = new Uint8Array(24 + 2 * n);
  b.set([0x43, 0x44, 0x57, 0x46, 1, 1, n & 0xff, n >> 8]);
  for (let i = 0; i < n; i++) {
    b[24 + i] = level(i);
    b[24 + n + i] = level(i) >> 1;
  }
  return b;
}

// jsdom lays nothing out: the waveform's box gets a size, everything else stays 0.
const sizes = { clientWidth: 400, clientHeight: 40 };
const originals: Partial<Record<keyof typeof sizes, PropertyDescriptor>> = {};
beforeAll(() => {
  for (const key of Object.keys(sizes) as (keyof typeof sizes)[]) {
    originals[key] = Object.getOwnPropertyDescriptor(Element.prototype, key);
    Object.defineProperty(Element.prototype, key, {
      configurable: true,
      get(this: Element) {
        return this.classList.contains("wave") ? sizes[key] : 0;
      },
    });
  }
});
afterAll(() => {
  for (const key of Object.keys(originals) as (keyof typeof sizes)[]) Object.defineProperty(Element.prototype, key, originals[key]!);
});
beforeEach(() => clearWaveforms());

function setup(waveform: (id: string) => unknown = () => waveBytes()) {
  const backend = mockBackend({ waveform: (a) => waveform(a.trackId as string) });
  const lib = makeLibrary(3);
  const queue = lib.tracks.map((t) => t.id);
  resetApp({ ...lib, status: makeStatus({ queue, index: 0, duration: 200, position: 50, playing: true }) });
  app.position = 50;
  return { backend, lib, queue };
}

const played = () => document.querySelector<HTMLElement>(".played");
const slider = () => screen.getByRole("slider", { name: "Seek" }) as HTMLInputElement;

describe("Now Playing's waveform seek bar", () => {
  test("draws the waveform; the played part follows the position with transforms only", async () => {
    app.waveformSeek = true;
    setup();
    render(NowPlaying);
    await settle();
    await settle();

    expect(document.querySelector(".wave .layer.rest")).not.toBeNull();
    expect(played()).not.toBeNull();
    // 50 of 200 s: the box is slid left by the 75% still to play, the picture slid back by as much.
    expect(played()!.style.transform).toBe("translateX(-75%)");
    expect(played()!.querySelector<HTMLElement>(".layer.now")!.style.transform).toBe("translateX(75%)");
    // The picture (a mask: --wave) is drawn once, at the box's size.
    const drawn = document.querySelector<HTMLElement>(".wave")!.dataset.drawn;
    expect(drawn).toBe("400x40");
    // The slider's own track isn't painted (no --pct to restyle every update).
    expect(slider().getAttribute("style") ?? "").not.toContain("--pct");

    app.position = 150;
    await settle();
    expect(played()!.style.transform).toBe("translateX(-25%)");
    expect(played()!.querySelector<HTMLElement>(".layer.now")!.style.transform).toBe("translateX(25%)");
    expect(document.querySelector<HTMLElement>(".wave")!.dataset.drawn).toBe(drawn);
  });

  test("without a waveform the plain line shows", async () => {
    app.waveformSeek = true;
    setup(() => new Uint8Array(0));
    render(NowPlaying);
    await settle();
    await settle();
    expect(document.querySelector(".wave .layer")).toBeNull();
    expect(slider().getAttribute("style")).toContain("--pct: 25%");
  });

  test("a damaged waveform shows the plain line", async () => {
    app.waveformSeek = true;
    const bad = waveBytes();
    bad[4] = 9;
    setup(() => bad);
    render(NowPlaying);
    await settle();
    await settle();
    expect(document.querySelector(".wave .layer")).toBeNull();
    expect(slider().getAttribute("style")).toContain("--pct");
  });

  test("asks once per song, never on the player's updates", async () => {
    app.waveformSeek = true;
    const { backend, queue } = setup();
    render(NowPlaying);
    await settle();
    expect(backend.to("waveform").map((c) => c.args.trackId)).toEqual([queue[0]]);

    for (let i = 1; i <= 6; i++) {
      await emit("player-status", makeStatus({ queue, index: 0, duration: 200, position: 50 + i / 4, playing: true }));
      app.status = makeStatus({ queue, index: 0, duration: 200, position: 50 + i / 4, playing: true });
      app.position = 50 + i / 4;
      await settle();
    }
    expect(backend.to("waveform")).toHaveLength(1);

    app.status = makeStatus({ queue, index: 1, duration: 200, position: 0, playing: true });
    await settle();
    app.status = makeStatus({ queue, index: 1, duration: 200, position: 0.25, playing: true });
    await settle();
    expect(backend.to("waveform").map((c) => c.args.trackId)).toEqual([queue[0], queue[1]]);

    // Back to the first song: remembered, not asked again.
    app.status = makeStatus({ queue, index: 0, duration: 200, position: 0, playing: true });
    await settle();
    expect(backend.to("waveform")).toHaveLength(2);
  });

  test("a waveform drawn in the background is fetched once when it's ready", async () => {
    app.waveformSeek = true;
    let ready = false;
    const { backend, queue } = setup(() => (ready ? waveBytes() : new Uint8Array(0)));
    render(NowPlaying);
    await settle();
    expect(document.querySelector(".wave .layer")).toBeNull();

    ready = true;
    await emit("waveform-ready", { trackId: "another-song", drawn: true });
    await settle();
    expect(backend.to("waveform")).toHaveLength(1);

    await emit("waveform-ready", { trackId: queue[0], drawn: true });
    await settle();
    await settle();
    expect(backend.to("waveform")).toHaveLength(2);
    expect(document.querySelector(".wave .layer.now")).not.toBeNull();
  });

  test("seeking by click still sends the same seek", async () => {
    app.waveformSeek = true;
    const { backend } = setup();
    render(NowPlaying);
    await settle();
    await settle();
    const s = slider();
    await fireEvent.pointerDown(s);
    await fireEvent.input(s, { target: { value: "120" } });
    await fireEvent.pointerUp(s);
    expect(backend.last("seek")).toEqual({ position: 120 });
    // The keyboard too (the slider's own change).
    await new Promise((r) => setTimeout(r, 160));
    await fireEvent.change(s, { target: { value: "130" } });
    expect(backend.last("seek")).toEqual({ position: 130 });
  });

  test("turned off: nothing is fetched and the line shows", async () => {
    app.waveformSeek = false;
    const { backend } = setup();
    render(NowPlaying);
    await settle();
    await settle();
    expect(backend.to("waveform")).toHaveLength(0);
    expect(document.querySelector(".wave")).toBeNull();
    expect(slider().getAttribute("style")).toContain("--pct: 25%");
    app.waveformSeek = true;
  });
});

describe("waveform data", () => {
  test("only whole files of format 1 are read", () => {
    const good = waveBytes(100);
    expect(parseWaveform(good)?.peak.length).toBe(100);
    expect(parseWaveform(good.buffer as ArrayBuffer)?.rms?.length).toBe(100);
    expect(parseWaveform(Array.from(good))?.peak[3]).toBe(21);
    expect(parseWaveform(good.slice(0, good.length - 1))).toBeNull();
    expect(parseWaveform(new Uint8Array(0))).toBeNull();
    expect(parseWaveform(null)).toBeNull();
    expect(parseWaveform(waveBytes(4))).toBeNull();
  });

  test("bars are scaled to the song: a quiet master fills the height, silence is flat", () => {
    const quiet = parseWaveform(waveBytes(200, (i) => 10 + (i % 10)))!;
    const h = barHeights(quiet, 50);
    expect(h).toHaveLength(50);
    expect(Math.max(...h)).toBeCloseTo(1, 5);
    expect(Math.min(...h)).toBeGreaterThan(0.4);
    const silent = parseWaveform(waveBytes(200, () => 0))!;
    expect(barHeights(silent, 50).every((v) => v === 0)).toBe(true);
    // No room: nothing drawn.
    expect(waveMask(quiet, 10, 40)).toBeNull();
    expect(waveMask(quiet, 400, 40)).toMatch(/^url\(/);
  });
});

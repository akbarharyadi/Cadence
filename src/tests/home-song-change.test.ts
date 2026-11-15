// Home on a 20,000-song library: a song change (once a second with very short songs, or shuffle
// skipping) costs what is on screen, never a pass over the whole library or history.
import { afterEach, describe, expect, test, vi } from "vitest";
import { flushSync } from "svelte";
import { render } from "@testing-library/svelte";
import { emit } from "@tauri-apps/api/event";
import Home from "#lib/views/Home.svelte";
import { rediscover } from "#lib/homePicks.js";
import { app } from "#lib/store.svelte.js";
import type { Album, Track } from "#lib/api.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

const ALBUMS = 2000;
const PER = 10;

/** 20,000 songs in 2,000 albums, and every pass over the album list counted. */
function bigLibrary() {
  const tracks: Track[] = [];
  const albums: Album[] = [];
  for (let a = 0; a < ALBUMS; a++) {
    const songs = Array.from({ length: PER }, (_, n) =>
      makeTrack({ id: `t${a}-${n}`, title: `Song ${n}`, artist: `Artist ${a % 300}`, album: `Album ${a}`, albumId: `al${a}`, trackNo: n + 1 }),
    );
    tracks.push(...songs);
    albums.push(makeAlbum(songs, { id: `al${a}`, title: `Album ${a}`, artist: `Artist ${a % 300}`, added: a }));
  }
  let passes = 0;
  const counted = new Proxy(albums, {
    get(target, key, receiver) {
      if (key === "filter" || key === "map" || key === "find" || key === "some" || key === "reduce" || key === Symbol.iterator) passes++;
      return Reflect.get(target, key, receiver);
    },
  });
  return { tracks, albums: counted as Album[], passes: () => passes };
}

afterEach(() => vi.useRealTimers());

/** A player update, as the store takes it in (it also notes the album as played this session). */
const play = (s: ReturnType<typeof makeStatus>) => (app as unknown as { applyStatus(s: unknown): void }).applyStatus(s);

describe("Home on a big library", () => {
  test("changing songs doesn't go over the whole library each time", async () => {
    const lib = bigLibrary();
    mockBackend({ get_history: () => ({ plays: {}, skips: {}, lastPlayed: {}, recentAlbums: [], totalListened: 0 }) });
    resetApp({ tracks: lib.tracks, albums: lib.albums });
    app.listening = { plays: {}, skips: {}, lastPlayed: {}, recentAlbums: ["al5", "al6", "al7"], totalListened: 0 } as never;
    const queue = lib.tracks.map((t) => t.id);
    app.status = makeStatus({ queue, index: 0, playing: true, duration: 1 });
    render(Home);
    flushSync();
    const before = lib.passes();
    const started = performance.now();
    const N = 100;
    // Shuffle: every song from another album.
    for (let i = 1; i <= N; i++) {
      play(makeStatus({ queue, index: ((i * 37) % ALBUMS) * PER, playing: true, duration: 1 }));
      flushSync();
    }
    const ms = (performance.now() - started) / N;
    const passes = lib.passes() - before;
    console.log(`Home, 20,000 songs: ${ms.toFixed(2)} ms and ${(passes / N).toFixed(1)} passes over the albums per song change`);
    expect(passes / N).toBeLessThan(1);
    expect(ms).toBeLessThan(15);
  });

  test("a burst of finished plays reloads the history once or twice, not once per play", async () => {
    const backend = mockBackend({
      get_library: () => ({ tracks: [], albums: [] }),
      get_folders: () => [],
      get_history: () => ({ plays: {}, skips: {}, lastPlayed: {}, recentAlbums: [], totalListened: 0 }),
      get_playlists: () => [],
      get_status: () => null,
    });
    resetApp();
    await app.init().catch(() => {});
    await settle();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    const loads = () => backend.to("get_history").length;
    const atStart = loads();
    // A play ending every 300 ms for 3 s.
    for (let i = 0; i < 10; i++) {
      await emit("history-changed", null);
      await vi.advanceTimersByTimeAsync(300);
    }
    await vi.advanceTimersByTimeAsync(5000);
    const reloads = loads() - atStart;
    expect(reloads).toBeGreaterThanOrEqual(1);
    expect(reloads).toBeLessThanOrEqual(3);
  });

  test("Rediscover changes by the day and skips the albums already on the cards before it", () => {
    const list = ["a", "b", "c", "d"].map((id) => ({ id }));
    expect(rediscover(list, 1, [])?.id).toBe("b");
    expect(rediscover(list, 2, [])?.id).toBe("c");
    expect(rediscover(list, 1, ["b", "c"])?.id).toBe("d");
    expect(rediscover([{ id: "a" }], 0, ["a"])).toBeUndefined();
    expect(rediscover([], 3, [])).toBeUndefined();
  });
});

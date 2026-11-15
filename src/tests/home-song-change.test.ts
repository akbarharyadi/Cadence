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

/** Whole-list operations: each one is a pass over the list. */
const PASSES = new Set<PropertyKey>(["filter", "map", "find", "findIndex", "some", "every", "reduce", "forEach", "flatMap", "indexOf", "includes", "slice", "sort", Symbol.iterator]);

/** `list`, with every pass over it counted in `tally`. */
function counted<T extends object>(list: T[], tally: { n: number }): T[] {
  return new Proxy(list, {
    get(target, key, receiver) {
      if (PASSES.has(key)) tally.n++;
      return Reflect.get(target, key, receiver);
    },
  });
}

/** 20,000 songs in 2,000 albums, and every pass over the album list and the song list counted. */
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
  const overAlbums = { n: 0 };
  const overSongs = { n: 0 };
  return { tracks: counted(tracks, overSongs), albums: counted(albums, overAlbums), passes: () => overAlbums.n, songPasses: () => overSongs.n };
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
    const songsBefore = lib.songPasses();
    const N = 100;
    // Shuffle: every song from another album.
    for (let i = 1; i <= N; i++) {
      play(makeStatus({ queue, index: ((i * 37) % ALBUMS) * PER, playing: true, duration: 1 }));
      flushSync();
    }
    // What a change costs, counted rather than timed (a busy machine slows a timing, not this):
    // what is on screen, never a pass over the whole library.
    const passes = lib.passes() - before;
    const songPasses = lib.songPasses() - songsBefore;
    expect(passes / N).toBeLessThan(1);
    expect(songPasses / N).toBeLessThan(1);
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

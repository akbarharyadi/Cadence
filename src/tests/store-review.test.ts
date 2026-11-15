// The store's answers to a review: a backup restore reloads what it changed, a device switch never
// carries one device's sound settings into another's, the artists and the search keys are asked
// for only when needed, and the merged artist is used where an artist is looked up by name.
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { ArtistIdentity, DspSettings, SearchKeys, Status } from "#lib/api.js";
import { overlays } from "#lib/palette.svelte.js";
import { app, coalesced } from "#lib/store.svelte.js";
import { reinsert } from "#lib/views/PlaylistView.svelte";
import { makeAlbum, makePlaylist, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

function dsp(preamp: number, crossfade = 4): DspSettings {
  return {
    eq: { enabled: true, preamp } as unknown as DspSettings["eq"],
    leveling: { mode: "auto", preamp: 0 },
    crossfeed: { enabled: false, level: "default" },
    exclusive: false,
    crossfade: { enabled: true, seconds: crossfade },
    tone: { loudness: false, smart: false },
    vocalCut: 0,
  };
}

/** A "player-status" update, as the store applies it. */
const playerStatus = (s: Status) => (app as unknown as { applyStatus(s: Status): void }).applyStatus(s);
const onDevice = (id: string, profile = 1, over: Partial<Status> = {}) => makeStatus({ device: { id, name: id, chosen: true, profile }, ...over });

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

afterEach(() => {
  vi.useRealTimers();
  overlays.paletteOpen = false;
});

describe("After a backup restore", () => {
  test("everything the restore changed is read again, and a slider touch then sends the restored settings", async () => {
    vi.useFakeTimers();
    const t = makeTrack();
    const backend = mockBackend({
      get_library: () => ({ tracks: [t], albums: [makeAlbum([t])] }),
      get_folders: () => ["D:\\Music"],
      get_status: () => makeStatus({ queue: [t.id], index: 0, duration: 200 }),
      get_dsp: () => dsp(-6, 9),
      get_history: () => ({ plays: { [t.id]: 3 }, skips: {}, lastPlayed: {}, recentAlbums: [], totalListened: 0 }),
      get_playlists: () => [makePlaylist([t.id])],
      get_marks: () => ({ [t.id]: { rating: 5, loved: true } }),
      get_skips: () => ({ ids: [t.id], instrumentals: true }),
      get_ai_settings: () => ({ settings: { provider: "off" }, hasKey: false }),
      artist_images: () => ({}),
      get_sleep_timer: () => null,
      attention_state: () => ({ ignored: ["x"], hidden: [] }),
    });
    resetApp();
    app.dsp = dsp(3, 4);
    // A slider moved just before: its 40 ms send is still waiting.
    app.updateDsp((d) => (d.eq.preamp = 2));
    const before = app.restored;
    await app.reloadAfterRestore();
    await vi.advanceTimersByTimeAsync(100);
    // The stale edit is never sent over what was restored.
    expect(backend.to("set_dsp")).toHaveLength(0);
    expect(app.dsp?.eq.preamp).toBe(-6);
    expect(app.dsp?.crossfade.seconds).toBe(9);
    expect(app.folders).toEqual(["D:\\Music"]);
    expect(app.playlists).toHaveLength(1);
    expect(app.isLoved(t)).toBe(true);
    expect(app.skipped).toEqual([t.id]);
    expect(app.skipInstrumentals).toBe(true);
    expect(app.listening?.plays[t.id]).toBe(3);
    expect(app.ai?.settings.provider).toBe("off");
    expect(app.attentionPrefs.ignored).toEqual(["x"]);
    expect(app.status?.queue).toEqual([t.id]);
    expect(app.restored).toBe(before + 1);

    // The next touch of a Sound control carries the restored settings, not the old ones.
    app.updateDsp((d) => (d.crossfade.enabled = false));
    await vi.advanceTimersByTimeAsync(50);
    const sent = backend.last("set_dsp")?.dsp as DspSettings;
    expect(sent.eq.preamp).toBe(-6);
    expect(sent.crossfade.seconds).toBe(9);
  });
});

describe("Switching the output device while a slider moves", () => {
  beforeEach(() => vi.useFakeTimers());

  test("the old device's waiting edit is dropped, edits wait for the new device's settings", async () => {
    const reply = deferred<DspSettings>();
    const backend = mockBackend({ get_dsp: () => reply.promise });
    resetApp();
    app.dsp = dsp(3);
    playerStatus(onDevice("dac"));
    app.updateDsp((d) => (d.eq.preamp = 5));
    // The device changes before the 40 ms send.
    playerStatus(onDevice("hdmi"));
    await vi.advanceTimersByTimeAsync(100);
    expect(backend.to("set_dsp")).toHaveLength(0);
    // Until the new device's settings are here, an edit isn't made (or sent).
    app.updateDsp((d) => (d.eq.preamp = 6));
    await vi.advanceTimersByTimeAsync(100);
    expect(backend.to("set_dsp")).toHaveLength(0);
    reply.resolve(dsp(-2));
    await vi.advanceTimersByTimeAsync(0);
    expect(app.dsp?.eq.preamp).toBe(-2);
    // Then edits go to the new device, with its settings.
    app.updateDsp((d) => (d.crossfade.seconds = 7));
    await vi.advanceTimersByTimeAsync(50);
    expect((backend.last("set_dsp")?.dsp as DspSettings).eq.preamp).toBe(-2);
  });

  test("the new settings wait for the pointer to be let go (no snap under the finger)", async () => {
    mockBackend({ get_dsp: () => dsp(-2) });
    resetApp();
    app.dsp = dsp(3);
    playerStatus(onDevice("dac"));
    (app as unknown as { watchPointer(): void }).watchPointer();
    window.dispatchEvent(new Event("pointerdown"));
    playerStatus(onDevice("dac", 2));
    await vi.advanceTimersByTimeAsync(0);
    expect(app.dsp?.eq.preamp).toBe(3);
    window.dispatchEvent(new Event("pointerup"));
    expect(app.dsp?.eq.preamp).toBe(-2);
  });

  test("an older answer for the settings never replaces a newer one", async () => {
    const first = deferred<DspSettings>();
    const second = deferred<DspSettings>();
    const replies = [first.promise, second.promise];
    mockBackend({ get_dsp: () => replies.shift() });
    resetApp();
    app.dsp = dsp(3);
    playerStatus(onDevice("a"));
    playerStatus(onDevice("b"));
    playerStatus(onDevice("c"));
    second.resolve(dsp(-9));
    await vi.advanceTimersByTimeAsync(0);
    first.resolve(dsp(1));
    await vi.advanceTimersByTimeAsync(0);
    expect(app.dsp?.eq.preamp).toBe(-9);
  });
});

describe("The artists and the search keys", () => {
  const identity = (version: number): ArtistIdentity => ({ version, groups: [{ names: ["Kenshi Yonezu", "米津玄師"], aliases: [], mbid: null, sort: null, native: null, latin: null, lang: null }] });
  const keys: SearchKeys = { version: 1, credits: { 米津玄師: "yonezu" }, tracks: {}, albums: {}, pending: false };

  test("an identity with the version already held is not applied again (the artists aren't rebuilt)", async () => {
    mockBackend({ artist_identity: () => identity(4) });
    resetApp();
    app.identity = identity(4);
    const held = app.identity;
    await app.loadIdentity();
    expect(app.identity).toBe(held);
    // An older one (a late answer) is ignored too.
    mockBackend({ artist_identity: () => identity(2) });
    await app.loadIdentity();
    expect(app.identity).toBe(held);
    mockBackend({ artist_identity: () => identity(5) });
    await app.loadIdentity();
    expect(app.identity?.version).toBe(5);
  });

  test("a burst of requests is one running and one more after it", async () => {
    const gate = deferred<ArtistIdentity>();
    let n = 0;
    const backend = mockBackend({ artist_identity: () => (++n === 1 ? gate.promise : identity(9)) });
    resetApp();
    const runs = [app.loadIdentity(), app.loadIdentity(), app.loadIdentity(), app.loadIdentity()];
    await settle();
    expect(backend.to("artist_identity")).toHaveLength(1);
    gate.resolve(identity(8));
    await Promise.all(runs);
    expect(backend.to("artist_identity")).toHaveLength(2);
    expect(app.identity?.version).toBe(9);
  });

  test("search keys: not at startup, once when Search or the palette first needs them, again only when out of date", async () => {
    const backend = mockBackend({ search_keys: () => keys });
    resetApp();
    const outdated = (now = false) => (app as unknown as { searchKeysOutdated(now?: boolean): void }).searchKeysOutdated(now);
    // A library change before anyone searched: nothing is fetched.
    outdated();
    await settle();
    expect(backend.to("search_keys")).toHaveLength(0);
    app.wantSearchKeys();
    await settle();
    expect(backend.to("search_keys")).toHaveLength(1);
    expect(app.searchKeys).toEqual(keys);
    // Opening again with nothing changed: no new request.
    app.wantSearchKeys();
    await settle();
    expect(backend.to("search_keys")).toHaveLength(1);
    // Changed while neither is on screen: only marked, fetched at the next open.
    outdated();
    await settle();
    expect(backend.to("search_keys")).toHaveLength(1);
    app.wantSearchKeys();
    await settle();
    expect(backend.to("search_keys")).toHaveLength(2);
    // The library changed while the palette is open: only marked (downloads change it often)…
    overlays.paletteOpen = true;
    outdated();
    await settle();
    expect(backend.to("search_keys")).toHaveLength(2);
    // …but new readings ("search-keys-changed") while it's open are fetched now.
    outdated(true);
    await settle();
    expect(backend.to("search_keys")).toHaveLength(3);
    // And not when neither is on screen.
    overlays.paletteOpen = false;
    outdated(true);
    await settle();
    expect(backend.to("search_keys")).toHaveLength(3);
  });

  test("the library is read once at a time: a burst while one runs is one more read", async () => {
    const gate = deferred<{ tracks: never[]; albums: never[] }>();
    let n = 0;
    const backend = mockBackend({ get_library: () => (++n === 1 ? gate.promise : { tracks: [], albums: [] }) });
    resetApp();
    const runs = [app.loadLibrary(), app.loadLibrary(), app.loadLibrary()];
    await settle();
    expect(backend.to("get_library")).toHaveLength(1);
    gate.resolve({ tracks: [], albums: [] });
    await Promise.all(runs);
    expect(backend.to("get_library")).toHaveLength(2);
    // Neither asks for the artists or the search keys.
    expect(backend.to("search_keys")).toHaveLength(0);
    expect(backend.to("artist_identity")).toHaveLength(0);
  });

  test("coalesced passes the last run's failure on, and runs again later", async () => {
    let fail = true;
    const run = coalesced(async () => {
      if (fail) throw new Error("no");
    });
    await expect(run()).rejects.toThrow("no");
    fail = false;
    await expect(run()).resolves.toBeUndefined();
  });
});

describe("One artist under several spellings, wherever an artist is looked up", () => {
  const song = (title: string, artist: string) => makeTrack({ title, artist, albumArtist: artist, album: title, albumId: `${artist}|${title}` });

  test("Stats' Play on an artist row plays the songs of every spelling; Go to Artist opens one page", () => {
    mockBackend();
    const lemon = song("Lemon", "米津玄師");
    const kick = song("Kick Back", "Kenshi Yonezu");
    const other = song("Halo", "Beyoncé");
    resetApp({ tracks: [lemon, kick, other], albums: [makeAlbum([lemon]), makeAlbum([kick]), makeAlbum([other])] });
    app.identity = { version: 1, groups: [{ names: ["Kenshi Yonezu", "米津玄師"], aliases: [], mbid: null, sort: null, native: null, latin: null, lang: null }] };
    expect(app.songsOfArtist("Kenshi Yonezu").map((t) => t.title).sort()).toEqual(["Kick Back", "Lemon"]);
    expect(app.songsOfArtist("米津玄師").map((t) => t.title).sort()).toEqual(["Kick Back", "Lemon"]);
    expect(app.artistOf("米津玄師")).toBe(app.artistOf("Kenshi Yonezu"));
    // Without the app's word they stay apart.
    app.identity = null;
    expect(app.songsOfArtist("米津玄師").map((t) => t.title)).toEqual(["Lemon"]);
  });
});

describe("Search from the sidebar", () => {
  test("a page change that throws never leaves the search box unable to open Search", () => {
    mockBackend();
    resetApp();
    const scroll = Element.prototype.scrollTo;
    // The page change fails once (anything in it throwing).
    Element.prototype.scrollTo = () => {
      throw new Error("boom");
    };
    const content = document.createElement("div");
    content.className = "content";
    document.body.appendChild(content);
    try {
      app.search = "a";
      expect(() => app.go({ name: "search", query: "a" })).toThrow("boom");
    } finally {
      Element.prototype.scrollTo = scroll;
    }
    app.view = { name: "home" };
    app.go({ name: "search", query: "ab" });
    expect(app.view).toEqual({ name: "search", query: "a" });
    content.remove();
  });
});

describe("Undo after removing songs from a playlist", () => {
  test("puts them back at their places in the list as it is now", () => {
    // a b c d e: b and d removed → a c e; then f added and e moved first → e a c f.
    expect(reinsert(["e", "a", "c", "f"], [{ pos: 1, id: "b" }, { pos: 3, id: "d" }])).toEqual(["e", "b", "a", "d", "c", "f"]);
    // Nothing changed since: exactly as before.
    expect(reinsert(["a", "c", "e"], [{ pos: 3, id: "d" }, { pos: 1, id: "b" }])).toEqual(["a", "b", "c", "d", "e"]);
    // The list got shorter: at its end.
    expect(reinsert([], [{ pos: 7, id: "x" }])).toEqual(["x"]);
  });
});

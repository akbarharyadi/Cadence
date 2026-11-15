// The mini player holds only what it shows, and the big library data isn't wrapped in proxies.
import { describe, expect, it, beforeEach } from "vitest";
import { emit } from "@tauri-apps/api/event";
import { app } from "#lib/store.svelte.js";
import { makeLibrary, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

function miniBackend(count = 5) {
  const lib = makeLibrary(count);
  let status = makeStatus({ queue: lib.tracks.map((t) => t.id), index: 1, playing: true, position: 10, duration: 200 });
  const backend = mockBackend({
    get_library: () => lib,
    get_library_part: (a) => ({ tracks: lib.tracks.filter((t) => (a.trackIds as string[]).includes(t.id)), albums: lib.albums }),
    get_status: () => status,
  });
  return { lib, backend, setStatus: (over: Parameters<typeof makeStatus>[0]) => (status = makeStatus({ ...status, ...over })) };
}

describe("the mini player", () => {
  beforeEach(() => resetApp());

  it("asks for the song playing and the next one, never the whole library", async () => {
    const { lib, backend } = miniBackend();
    await app.initMini();
    expect(backend.to("get_library")).toHaveLength(0);
    expect(backend.last("get_library_part")?.trackIds).toEqual([lib.tracks[1].id, lib.tracks[2].id]);
    expect(app.tracks).toHaveLength(2);
    expect(app.current?.id).toBe(lib.tracks[1].id);
    expect(app.currentAlbum?.id).toBe(lib.albums[0].id);
  });

  it("asks again only when the song changes or the library does", async () => {
    const { lib, backend, setStatus } = miniBackend();
    await app.initMini();
    const asked = () => backend.to("get_library_part").length;
    const before = asked();
    // Position updates of the same song: nothing to fetch.
    await emit("player-status", makeStatus({ ...app.status!, position: 11 }));
    await emit("player-status", makeStatus({ ...app.status!, position: 12 }));
    await settle();
    expect(asked()).toBe(before);
    // The next song: it was fetched already, so it shows at once; then the one after it is asked for.
    await emit("player-status", setStatus({ index: 2 }));
    expect(app.current?.id).toBe(lib.tracks[2].id);
    await settle();
    expect(backend.last("get_library_part")?.trackIds).toEqual([lib.tracks[2].id, lib.tracks[3].id]);
    // The library changed (a tag edit): asked again for the same songs.
    const n = asked();
    await emit("library-changed", null);
    await settle();
    expect(asked()).toBe(n + 1);
  });

  it("doesn't follow the main window's events", async () => {
    const { backend } = miniBackend();
    await app.initMini();
    const before = backend.calls.length;
    // Each of these makes the main window ask the app for something.
    await emit("marks-changed", null);
    await emit("history-changed", null);
    await emit("sonic-progress", { done: 1, total: 1 });
    await emit("health-done", "quality");
    await emit("playlists-changed", null);
    await settle();
    expect(backend.calls.slice(before).map((c) => c.cmd)).toEqual([]);
  });
});

describe("library data held by the window", () => {
  beforeEach(() => resetApp());

  it("keeps the songs, albums and marks as given, without deep proxies", () => {
    mockBackend();
    const { tracks, albums } = makeLibrary(3, [makeTrack()]);
    app.tracks = tracks;
    app.albums = albums;
    const marks = { [tracks[0].id]: { rating: 5, loved: true } };
    app.marks = marks;
    // A deeply reactive field hands back a proxy, with a signal per field read on each song.
    expect(app.tracks).toBe(tracks);
    expect(app.tracks[0]).toBe(tracks[0]);
    expect(app.albums).toBe(albums);
    expect(app.marks).toBe(marks);
    // Changes still show: they replace the whole value.
    app.setLoved([tracks[1]], true);
    expect(app.isLoved(tracks[1])).toBe(true);
    expect(app.favorites.map((t) => t.id)).toContain(tracks[1].id);
  });
});

import { fireEvent, render, screen } from "@testing-library/svelte";
import { flushSync } from "svelte";
import { describe, expect, test } from "vitest";
import TrackList from "#lib/components/TrackList.svelte";
import Songs from "#lib/views/Songs.svelte";
import { app } from "#lib/store.svelte.js";
import { KEEP, MARGIN, nextRange, type Range } from "#lib/virtualWindow.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp } from "./helpers";

// A 20,000-song library: what the Songs list builds and redoes while it scrolls, selects and plays.
const N = 20_000;
const library = () => {
  const tracks = Array.from({ length: N }, (_, i) => makeTrack({ title: `Song ${String(N - i).padStart(5, "0")}` }));
  return { tracks, albums: [makeAlbum(tracks)] };
};
const ROW = 54;

describe("The rows a long list builds", () => {
  test("scrolling row by row moves the built rows in steps, not on every frame", () => {
    let range: Range = { first: 0, last: 0 };
    let moves = 0;
    // 2,000 rows scrolled a few pixels at a time, on a 900 px window.
    for (let y = 0; y <= 2000 * ROW; y += 9) {
      const next = nextRange(range, { top: -y, height: 900, count: N, row: ROW });
      if (next !== range) moves++;
      range = next;
      // What's on screen is always built.
      expect(range.first).toBeLessThanOrEqual(Math.floor(y / ROW));
      expect(range.last).toBeGreaterThanOrEqual(Math.min(N, Math.ceil((y + 900) / ROW)));
    }
    // One move per (MARGIN - KEEP) rows scrolled, at most (it was one per row).
    expect(moves).toBeLessThanOrEqual(Math.ceil(2000 / (MARGIN - KEEP)) + 1);
    expect(range.last - range.first).toBeLessThanOrEqual(Math.ceil(900 / ROW) + 1 + 2 * MARGIN);
  });

  test("a list that got shorter is built again within its end", () => {
    const r = nextRange({ first: 100, last: 140 }, { top: -50 * ROW, height: 900, count: 60, row: ROW });
    expect(r.last).toBeLessThanOrEqual(60);
    expect(r.first).toBeLessThanOrEqual(50);
  });

  test("20,000 songs build a screenful of rows, and only the rated or active rows build their stars", () => {
    mockBackend();
    const lib = library();
    resetApp(lib);
    app.marks = { [lib.tracks[1].id]: { rating: 4, loved: false } } as typeof app.marks;
    render(TrackList, { tracks: lib.tracks, showArt: true, showAlbum: true });
    const rows = screen.getAllByRole("option");
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.length).toBeLessThanOrEqual(60);
    // One star group: the rated song (no row is pointed at or active yet).
    expect(screen.getAllByRole("group", { name: /^Rating for/ })).toHaveLength(1);
    // Each row stays small.
    const elements = document.querySelectorAll("[role=listbox] *").length;
    expect(elements / rows.length).toBeLessThan(30);
  });
});

describe("Selecting and playing in a long list", () => {
  test("Ctrl+A selects 20,000 songs at once, quickly, and the built rows show it", () => {
    mockBackend();
    const lib = library();
    resetApp(lib);
    render(TrackList, { tracks: lib.tracks });
    const list = screen.getByRole("listbox");
    const t0 = performance.now();
    fireEvent.keyDown(list, { key: "a", ctrlKey: true });
    flushSync();
    expect(performance.now() - t0).toBeLessThan(250);
    const rows = screen.getAllByRole("option");
    expect(rows.every((r) => r.getAttribute("aria-selected") === "true")).toBe(true);
    expect(screen.getByRole("toolbar", { name: "Selected songs" })).toHaveTextContent(/20\D?000 selected/);
  });

  test("player updates and a song change keep the built rows; only the playing row's look changes", () => {
    mockBackend();
    const lib = library();
    resetApp(lib);
    render(TrackList, { tracks: lib.tracks });
    const before = screen.getAllByRole("option");
    const queue = lib.tracks.map((t) => t.id);
    app.status = makeStatus({ queue, index: 2, playing: true, position: 1 });
    flushSync();
    for (let p = 1.25; p < 3; p += 0.25) {
      app.status = makeStatus({ queue, index: 2, playing: true, position: p });
      flushSync();
    }
    app.status = makeStatus({ queue, index: 3, playing: true, position: 0 });
    flushSync();
    const after = screen.getAllByRole("option");
    expect(after).toHaveLength(before.length);
    after.forEach((r, i) => expect(r).toBe(before[i]));
    expect(after.filter((r) => r.classList.contains("current")).map((r) => r.dataset.rowI)).toEqual(["3"]);
  });

  test("the Songs page sorts 20,000 songs by title within a time bound", () => {
    mockBackend();
    const lib = library();
    resetApp(lib);
    const t0 = performance.now();
    render(Songs);
    expect(performance.now() - t0).toBeLessThan(1500);
    const first = screen.getAllByRole("option")[0];
    expect(first).toHaveTextContent("Song 00001");
  });
});

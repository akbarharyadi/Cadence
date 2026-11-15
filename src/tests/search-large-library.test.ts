import { render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { flushSync } from "svelte";
import Search from "#lib/views/Search.svelte";
import type { Album, Track } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";
import { albumFields, artistFields, fold, isFolded, matchingNames, ranked, trackFields, warm } from "#lib/searchIndex.js";
import { search } from "#lib/paletteItems.js";
import { warmSearch } from "#lib/searchWarm.js";
import { makeAlbum, makeTrack, mockBackend, resetApp } from "./helpers";

// A big library: 20,000 songs on 2,000 albums by 300 artists, invented names in several scripts.
const WORDS = ["Summer", "Rain", "Paper", "Lights", "Orbit", "Hearts", "Midnight", "Days", "Fire", "Glass", "River", "Echo", "雨上がり", "夜明け", "첫눈", "梦想", "Café", "Ölmez"];
function bigLibrary(): { tracks: Track[]; albums: Album[] } {
  const tracks: Track[] = [];
  const albums: Album[] = [];
  for (let a = 0; a < 2000; a++) {
    const artist = `${WORDS[a % 7]}${WORDS[(a * 3) % 11].toLowerCase()} ${a % 300}`;
    const album = `${WORDS[a % WORDS.length]} ${WORDS[(a + 5) % WORDS.length]} Vol.${a}`;
    const songs = Array.from({ length: 10 }, (_, i) =>
      makeTrack({ title: `${WORDS[(a + i) % WORDS.length]} ${WORDS[(a * 7 + i) % WORDS.length]} #${i}`, artist, albumArtist: artist, album, albumId: `al${a}`, trackNo: i + 1 }),
    );
    tracks.push(...songs);
    albums.push(makeAlbum(songs));
  }
  return { tracks, albums };
}

/** Other names for every artist and readings for some titles, as the app sends them. */
function keys() {
  const credits: Record<string, string> = {};
  for (const t of app.tracks) credits[t.artist] = `${fold(t.artist)} alias${t.artist.length}`;
  const tracks: Record<string, string> = {};
  app.tracks.forEach((t, i) => {
    if (i % 14 === 0) tracks[t.id] = "ameagari yoake";
  });
  return { version: 1, credits, tracks, albums: {}, pending: false };
}

const ms = (f: () => void) => {
  const t = performance.now();
  f();
  return performance.now() - t;
};
const median = (xs: number[]) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];
/** Runs the idle slices of `warm` until the lists are folded. */
async function warmed() {
  warmSearch();
  for (let i = 0; i < 500 && !(isFolded(app.tracks) && isFolded(app.albums) && isFolded(app.artists)); i++) await vi.advanceTimersByTimeAsync(1);
}

// Only the timers are fake: the clock stays real, so the time bounds measure real work.
beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  mockBackend();
  resetApp(bigLibrary());
});
afterEach(() => vi.useRealTimers());

const QUERIES = ["s", "su", "summer", "summer r", "summer rain", "midnight l", "雨", "cafe", "zzz", "alias1"];

describe("Search on 20,000 songs", () => {
  test("the index is made in idle slices, none of them long, before the first key", async () => {
    expect(isFolded(app.tracks)).toBe(false);
    const idle = (globalThis as { requestIdleCallback?: unknown }).requestIdleCallback;
    // (jsdom has no idle callbacks: the slices come from timers.)
    expect(idle).toBeUndefined();
    const slices: number[] = [];
    warm(app.tracks, trackFields);
    for (let i = 0; i < 500 && !isFolded(app.tracks); i++) {
      const t = performance.now();
      await vi.advanceTimersByTimeAsync(1);
      slices.push(performance.now() - t);
    }
    expect(isFolded(app.tracks)).toBe(true);
    expect(slices.length).toBeGreaterThan(5);
    expect(median(slices), `slices ${slices.map((s) => s.toFixed(1)).join(" ")}`).toBeLessThan(40);
  });

  test("a query on the made index takes a few milliseconds", async () => {
    await warmed();
    app.searchKeys = keys();
    const times = QUERIES.map((q) => ms(() => matchingNames(app.tracks, trackFields, fold(q), 200)));
    const ranks = QUERIES.map((q) => ms(() => ranked(app.tracks, trackFields, fold(q), 6)));
    expect(median(times), times.join(" ")).toBeLessThan(10);
    expect(Math.max(...times), times.join(" ")).toBeLessThan(40);
    expect(median(ranks), ranks.join(" ")).toBeLessThan(10);
  });

  test("the command palette's search for a key stays small", async () => {
    await warmed();
    app.searchKeys = keys();
    search("warm", [], []); // the store's artists, worked out once
    const times = QUERIES.map((q) => ms(() => search(q, [], [])));
    expect(median(times), times.join(" ")).toBeLessThan(15);
    expect(Math.max(...times), times.join(" ")).toBeLessThan(50);
    // Its songs, found by an artist's other name too.
    const found = search("alias1", [], []).find((g) => g.kind === "song");
    expect(found?.items.length).toBeGreaterThan(0);
  });

  test("the first key opens the page without making the index on the way", async () => {
    render(Search, { query: "summer" });
    // Not yet: the pause after the key comes first (the index is made meanwhile).
    expect(screen.queryAllByRole("group", { name: / by / }).length).toBe(0);
    expect(isFolded(app.tracks)).toBe(false);
    await warmed();
    await vi.advanceTimersByTimeAsync(200);
    flushSync();
    expect(screen.queryAllByRole("group", { name: / by / }).length).toBeGreaterThan(0);
  });

  test("a search box getting focus starts making the index", async () => {
    const box = document.createElement("input");
    box.setAttribute("data-global-search", "sidebar");
    document.body.append(box);
    box.focus();
    for (let i = 0; i < 500 && !isFolded(app.tracks); i++) await vi.advanceTimersByTimeAsync(1);
    expect(isFolded(app.tracks)).toBe(true);
    box.remove();
  });

  test("thousands of matches show a bounded number of rows and album cards", async () => {
    await warmed();
    const { rerender } = render(Search, { query: "" });
    await rerender({ query: "summer" });
    await vi.advanceTimersByTimeAsync(200);
    flushSync();
    const cards = screen.queryAllByRole("group", { name: / by / });
    expect(cards.length).toBe(12);
    expect(screen.getByRole("button", { name: "Show more albums" })).toBeTruthy();
    // The songs list is virtual: rows on screen and a margin, not the 200 found.
    expect(screen.queryAllByRole("option").length).toBeLessThanOrEqual(60);
  });

  test("the other names arriving leave a full list of results as it is", async () => {
    await warmed();
    const { rerender } = render(Search, { query: "" });
    await rerender({ query: "summer" });
    await vi.advanceTimersByTimeAsync(200);
    flushSync();
    const before = screen.queryAllByRole("group", { name: / by / });
    app.searchKeys = keys();
    flushSync();
    const after = screen.queryAllByRole("group", { name: / by / });
    expect(after.length).toBe(before.length);
    after.forEach((el, i) => expect(el).toBe(before[i]));
  });

  test("albums and artists are matched with the same shared fields", () => {
    expect(albumFields(app.albums[0])).toEqual([app.albums[0].title, app.albums[0].artist]);
    expect(artistFields({ name: "X" })).toEqual(["X"]);
  });
});

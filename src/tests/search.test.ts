import { render, screen, within } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import Search from "#lib/views/Search.svelte";
import type { Album, Track } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeTrack, mockBackend, resetApp } from "./helpers";

// A library with accents, another script, and a subtitle: the things "accent-insensitive" is for.
const song = (title: string, artist: string, album: string, extra: Partial<Track> = {}) =>
  makeTrack({ title, artist, albumArtist: artist, album, albumId: `${artist}|${album}`, ...extra });

function library(): { tracks: Track[]; albums: Album[] } {
  const tracks = [
    song("Lemon", "Kenshi Yonezu", "Bootleg"),
    song("Kick Back", "Kenshi Yonezu", "Kick Back"),
    song("Lemon", "米津玄師", "Bootleg"),
    song("Pokémon Theme", "Various Artists", "Pokémon: Red & Blue"),
    song("Halo", "Beyoncé", "I Am... Sasha Fierce"),
    song("Hoppípolla", "Sigur Rós", "Takk..."),
    song("Kickstart My Heart", "Mötley Crüe", "Dr. Feelgood"),
    song("Other", "Someone Else", "Misc", { subtitle: "Yonezu Remix" }),
    song("Bohemian Rhapsody", "Queen", "A Night at the Opera"),
    song("Karma Police", "Radiohead", "OK Computer"),
    ...Array.from({ length: 25 }, (_, i) => song(`Filler ${i + 1}`, `Band ${i % 5}`, `Record ${i % 7}`)),
  ];
  const byAlbum = new Map<string, Track[]>();
  for (const t of tracks) byAlbum.set(t.albumId, [...(byAlbum.get(t.albumId) ?? []), t]);
  return { tracks, albums: [...byAlbum.values()].map((ts) => makeAlbum(ts)) };
}

// Written independently of the view: accents stripped with NFD and the combining-marks block.
const plain = (s: string) => s.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase();
const reference = (tracks: Track[], q: string) => {
  const needle = plain(q.trim());
  if (!needle) return [];
  return tracks.filter((t) => [t.title, t.subtitle ?? "", t.artist, t.album].some((f) => plain(f).includes(needle))).slice(0, 200);
};

/** What the Songs list shows: the title of each row, read from its Play button. */
function shownTitles(): string[] {
  const rows = screen.queryAllByRole("option");
  return rows.map((r) => within(r).getByRole("button", { name: /^Play / }).getAttribute("aria-label")!.replace(/^Play /, ""));
}
const settleSearch = () => vi.advanceTimersByTimeAsync(200); // the view waits 120 ms after the last key

beforeEach(() => {
  vi.useFakeTimers();
  mockBackend();
});
afterEach(() => vi.useRealTimers());

describe("Search", () => {
  test.each(["yonezu", "YONEZU", "  yonezu  ", "lemon", "pokemon", "Pokémon", "pokémon", "ros", "rós", "beyonce", "motley crue", "kick", "米津", "i am", "zzzz-no-such"])(
    "query %j finds the same songs as a plain filter",
    async (query) => {
      const lib = library();
      resetApp(lib);
      const { rerender } = render(Search, { query: "" });
      await rerender({ query });
      await settleSearch();

      const expected = reference(lib.tracks, query).map((t) => t.title);
      expect(shownTitles()).toEqual(expected);
    },
  );

  test("the subtitle, artist and album are searched too (not just the title)", async () => {
    const lib = library();
    resetApp(lib);
    render(Search, { query: "yonezu" });
    await settleSearch();
    // Two by the artist, and "Other", whose subtitle says Yonezu.
    expect(shownTitles()).toEqual(["Lemon", "Kick Back", "Other"]);
  });

  test("nothing is listed for an empty query (it would match the whole library)", async () => {
    resetApp(library());
    render(Search, { query: "   " });
    await settleSearch();
    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });

  test("results follow the library when its list is replaced (the folded-names cache is dropped)", async () => {
    const lib = library();
    resetApp(lib);
    render(Search, { query: "newcomer" });
    await settleSearch();
    expect(shownTitles()).toEqual([]);

    // A rescan replaces the whole list: one song added...
    const added = song("Fresh", "The Newcomer", "Debut");
    app.tracks = [...lib.tracks, added];
    await settleSearch();
    expect(shownTitles()).toEqual(["Fresh"]);

    // ...then another list of the same length where a song's artist changed.
    app.tracks = app.tracks.map((t) => (t.id === added.id ? { ...t, artist: "Somebody" } : t));
    await settleSearch();
    expect(shownTitles()).toEqual([]);
  });

  test("a query that changes shows the new results after the 120 ms pause, not before", async () => {
    const lib = library();
    resetApp(lib);
    const { rerender } = render(Search, { query: "queen" });
    await settleSearch();
    expect(shownTitles()).toEqual(["Bohemian Rhapsody"]);

    await rerender({ query: "radiohead" });
    expect(shownTitles()).toEqual(["Bohemian Rhapsody"]); // still the last query's
    await settleSearch();
    expect(shownTitles()).toEqual(["Karma Police"]);
  });
});

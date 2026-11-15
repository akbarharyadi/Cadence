// One artist across scripts: 米津玄師 and Kenshi Yonezu are one page, and search finds songs by
// any of an artist's names (the app works out which names are one artist and the extra words).
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import AppKeys from "#lib/components/AppKeys.svelte";
import ArtistView from "#lib/views/ArtistView.svelte";
import Search from "#lib/views/Search.svelte";
import type { Album, ArtistIdentity, SearchKeys, Track } from "#lib/api.js";
import { overlays } from "#lib/palette.svelte.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeTrack, mockBackend, resetApp } from "./helpers";

const song = (title: string, artist: string, album: string, extra: Partial<Track> = {}) =>
  makeTrack({ title, artist, albumArtist: artist, album, albumId: `${artist}|${album}`, ...extra });

function library(): { tracks: Track[]; albums: Album[] } {
  const tracks = [
    song("Lemon", "米津玄師", "Lemon"),
    song("Kick Back", "Kenshi Yonezu", "Kick Back"),
    song("Peace Sign", "Kenshi Yonezu", "Kick Back"),
    song("Other", "Someone Else", "Misc", { subtitle: "Yonezu Remix" }),
    song("Dynamite", "방탄소년단", "BE"),
    song("とうきょう", "りりあ。", "うた"),
    song("Halo", "Beyoncé", "I Am... Sasha Fierce"),
  ];
  const byAlbum = new Map<string, Track[]>();
  for (const t of tracks) byAlbum.set(t.albumId, [...(byAlbum.get(t.albumId) ?? []), t]);
  return { tracks, albums: [...byAlbum.values()].map((ts) => makeAlbum(ts)) };
}

const identity: ArtistIdentity = {
  version: 1,
  groups: [{ names: ["Kenshi Yonezu", "米津玄師"], aliases: ["よねづけんし"], mbid: "mb-1", sort: "Yonezu, Kenshi", native: "米津玄師", latin: "Kenshi Yonezu", lang: "japanese" }],
};

// As the app sends them: folded words, one per line.
const keys: SearchKeys = {
  version: 1,
  credits: {
    米津玄師: "kenshi yonezu\nよねつけんし\nyonezukenshi",
    "Kenshi Yonezu": "米津玄師\nよねつけんし",
    방탄소년단: "bts\nbangtan boys\nbangtansonyeondan\nbangtan",
    "りりあ。": "riria",
  },
  tracks: {},
  albums: {},
  pending: false,
};

function withKeys(lib: { tracks: Track[]; albums: Album[] }): SearchKeys {
  const tokyo = lib.tracks.find((t) => t.title === "とうきょう")!;
  return { ...keys, tracks: { [tokyo.id]: "tokyo\ntoukyou" } };
}

const settleSearch = () => vi.advanceTimersByTimeAsync(200);
function shownTitles(): string[] {
  return screen.queryAllByRole("option").map((r) => within(r).getByRole("button", { name: /^Play / }).getAttribute("aria-label")!.replace(/^Play /, ""));
}

describe("One artist under several spellings", () => {
  beforeEach(() => mockBackend());

  test("is one artist page with both spellings' albums and an “also written as” line", async () => {
    resetApp(library());
    app.identity = identity;
    // One entry in the artists list, under the spelling on most songs.
    expect(app.artists.filter((a) => /Yonezu|米津/.test(a.name)).map((a) => a.name)).toEqual(["Kenshi Yonezu"]);
    render(ArtistView, { name: "米津玄師" });
    expect(screen.getByRole("heading", { level: 1, name: "Kenshi Yonezu" })).toBeInTheDocument();
    expect(screen.getByText(/Also written as/)).toHaveTextContent("米津玄師");
    expect(screen.getByText(/3 songs/)).toBeInTheDocument();
    expect(screen.getAllByText("Lemon").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Kick Back").length).toBeGreaterThan(0);
  });

  test("without the app's word they stay two artists", () => {
    resetApp(library());
    expect(app.artists.filter((a) => /Yonezu|米津/.test(a.name)).map((a) => a.name).sort()).toEqual(["Kenshi Yonezu", "米津玄師"]);
  });

  test("“Not the same artist” splits the spelling out (and can be undone)", async () => {
    const backend = mockBackend({ artist_mark_different: () => [["米津玄師", "Kenshi Yonezu"]] });
    resetApp(library());
    app.identity = identity;
    render(ArtistView, { name: "Kenshi Yonezu" });
    await userEvent.setup().click(screen.getByRole("button", { name: "Not the same artist" }));
    await waitFor(() => expect(backend.last("artist_mark_different")).toEqual({ name: "米津玄師", others: ["Kenshi Yonezu"] }));
    expect(app.toast).toMatch(/米津玄師/);
    app.toastAction?.run();
    await waitFor(() => expect(backend.last("artist_forget")).toEqual({ pairs: [["米津玄師", "Kenshi Yonezu"]] }));
  });
});

describe("Search by other names", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockBackend();
  });
  afterEach(() => vi.useRealTimers());

  test("“yonezu” finds the songs tagged 米津玄師, after those whose names say Yonezu", async () => {
    const lib = library();
    resetApp(lib);
    app.searchKeys = withKeys(lib);
    render(Search, { query: "yonezu" });
    await settleSearch();
    expect(shownTitles()).toEqual(["Kick Back", "Peace Sign", "Other", "Lemon"]);
  });

  test("“bangtan” finds 방탄소년단, “tokyo” finds とうきょう; nothing without the keys", async () => {
    const lib = library();
    resetApp(lib);
    app.searchKeys = withKeys(lib);
    const { rerender } = render(Search, { query: "bangtan" });
    await settleSearch();
    expect(shownTitles()).toEqual(["Dynamite"]);
    expect(screen.getByRole("button", { name: "방탄소년단" })).toBeInTheDocument();
    await rerender({ query: "tokyo" });
    await settleSearch();
    expect(shownTitles()).toEqual(["とうきょう"]);
    app.searchKeys = null;
    await rerender({ query: "bangtan" });
    await settleSearch();
    expect(shownTitles()).toEqual([]);
  });
});

describe("Command palette by other names", () => {
  afterEach(() => (overlays.paletteOpen = false));

  test("finds a song and its artist by an alias", async () => {
    mockBackend();
    const user = userEvent.setup();
    const lib = library();
    resetApp(lib);
    app.searchKeys = withKeys(lib);
    render(AppKeys);
    await user.keyboard("{Control>}k{/Control}");
    await user.type(screen.getByRole("combobox", { name: /Search pages, actions/ }), "bangtan");
    expect(await screen.findByRole("option", { name: /^Dynamite/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /^방탄소년단/ })).toBeInTheDocument();
  });
});

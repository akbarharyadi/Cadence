import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import Stats from "#lib/views/Stats.svelte";
import type { ListeningStats } from "#lib/api.js";
import { makeAlbum, makeTrack, mockBackend, resetApp, settle } from "./helpers";

const lemon = makeTrack({ title: "Lemon", artist: "Kenshi Yonezu", albumId: "al1" });
const flamingo = makeTrack({ title: "Flamingo", artist: "Kenshi Yonezu", albumId: "al1" });
const album = makeAlbum([lemon, flamingo], { id: "al1", title: "STRAY SHEEP", artist: "Kenshi Yonezu" });

const base: ListeningStats = {
  range: "month",
  plays: 40,
  finished: 30,
  skipped: 5,
  minutes: 120,
  songs: 2,
  artists: 1,
  albums: 1,
  topSongs: [{ id: lemon.id, name: "Lemon", sub: "Kenshi Yonezu", finished: 20, plays: 25, minutes: 80 }],
  topArtists: [],
  topAlbums: [],
  timeline: [],
};

const full: ListeningStats = {
  ...base,
  onThisDay: [{ id: flamingo.id, name: "Flamingo", sub: "Kenshi Yonezu", year: 2024, plays: 3 }],
  forgotten: [{ id: lemon.id, name: "Lemon", sub: "Kenshi Yonezu", plays: 40, last: 1_700_000_000, loved: true, rating: 5 }],
  clock: Array.from({ length: 24 }, (_, h) => (h === 21 ? 30 : 1)),
  weekdays: [5, 5, 5, 5, 10, 20, 3],
  streak: { current: 4, longest: 12, longestFrom: "2025-03-01", longestTo: "2025-03-12", days: 200 },
  discovery: [
    { month: "2026-09", count: 2 },
    { month: "2026-10", count: 0 },
    { month: "2026-11", count: 5 },
  ],
  firstSong: { id: lemon.id, name: "Lemon", sub: "Kenshi Yonezu", at: 1_767_225_600 },
  completeAlbums: [{ id: "al1", name: "STRAY SHEEP", sub: "Kenshi Yonezu", finished: 2, plays: 2, minutes: 120 }],
  decades: [
    { decade: 2010, plays: 10, minutes: 40 },
    { decade: 2020, plays: 30, minutes: 80 },
  ],
  historyFrom: 1_600_000_000,
};

function open(answer: ListeningStats) {
  const backend = mockBackend({ listening_stats: () => answer });
  resetApp({ tracks: [lemon, flamingo], albums: [album] });
  render(Stats);
  return { backend, user: userEvent.setup() };
}

const section = (name: string) => screen.queryByRole("region", { name });

describe("Your Stats: sections over the whole history", () => {
  test("every new section renders from the backend's answer", async () => {
    open(full);
    await settle();
    expect(within(section("On this day")!).getByText("Flamingo")).toBeInTheDocument();
    expect(within(section("On this day")!).getByText("2024")).toBeInTheDocument();
    expect(within(section("Forgotten favourites")!).getByText("Lemon")).toBeInTheDocument();
    expect(section("When you listen")).not.toBeNull();
    expect(screen.getByLabelText("Plays by weekday")).toBeInTheDocument();
    expect(screen.getByText("12 days")).toBeInTheDocument();
    expect(screen.getByText(/First song of/)).toBeInTheDocument();
    expect(within(section("Albums heard through")!).getByText("STRAY SHEEP")).toBeInTheDocument();
    expect(within(section("Decades")!).getByText("2020s")).toBeInTheDocument();
    expect(section("New artists")).not.toBeNull();
  });

  test("“Play these” plays the section's songs", async () => {
    const { backend, user } = open(full);
    await settle();
    await user.click(within(section("Forgotten favourites")!).getByRole("button", { name: /Play these/ }));
    expect(backend.last("play_tracks")?.ids).toEqual([lemon.id]);
    await user.click(within(section("On this day")!).getByRole("button", { name: /Play these/ }));
    expect(backend.last("play_tracks")?.ids).toEqual([flamingo.id]);
  });

  test("sections without enough data stay hidden (and an older answer without them still renders)", async () => {
    open({
      ...base,
      onThisDay: [],
      forgotten: [],
      clock: Array(24).fill(0),
      weekdays: [1, 0, 0, 0, 0, 0, 0],
      streak: { current: 1, longest: 1, longestFrom: "2026-11-15", longestTo: "2026-11-15", days: 1 },
      discovery: [{ month: "2026-11", count: 3 }],
      firstSong: null,
      completeAlbums: [],
      decades: [{ decade: 2020, plays: 3, minutes: 9 }],
    });
    await settle();
    expect(screen.getByText("Lemon")).toBeInTheDocument();
    for (const name of ["On this day", "Forgotten favourites", "When you listen", "New artists", "Albums heard through", "Decades"]) {
      expect(section(name)).toBeNull();
    }
    expect(screen.queryByText(/longest streak/)).toBeNull();
    expect(screen.queryByText(/First song of/)).toBeNull();
  });

  test("an answer from before these sections existed renders the original page", async () => {
    open(base);
    await settle();
    expect(screen.getByText("Lemon")).toBeInTheDocument();
    expect(section("On this day")).toBeNull();
  });
});

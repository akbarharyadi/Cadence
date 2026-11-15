import { fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import Search from "#lib/views/Search.svelte";
import { app } from "#lib/store.svelte.js";
import type { Status } from "#lib/api.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp } from "./helpers";

/** A "player-status" update, as the store applies it. */
const playerStatus = (over: Partial<Status>) => (app as unknown as { applyStatus(s: Status): void }).applyStatus(makeStatus(over));

// Invented songs and lines (no real lyrics in fixtures).
const a = makeTrack({ title: "Test Song", artist: "Example Artist", albumId: "alx", album: "Example Album", trackNo: 1 });
const b = makeTrack({ title: "Second Song", artist: "Example Artist", albumId: "alx", album: "Example Album", trackNo: 2 });
const gone = "not-in-the-library";
const settleSearch = () => vi.advanceTimersByTimeAsync(200);

function open(hits: unknown, ready = true) {
  const backend = mockBackend({ lyric_search: () => ({ ready, hits }) });
  resetApp({ tracks: [a, b], albums: [makeAlbum([a, b])] });
  return backend;
}

beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
});
afterEach(() => vi.useRealTimers());

describe("Search: songs found by a line of their lyrics", () => {
  test("a Lyrics group lists the song with its matching line and time", async () => {
    const backend = open([
      { trackId: b.id, line: "the rain is falling today", time: 42.5 },
      { trackId: gone, line: "the rain is falling", time: 3 },
    ]);
    render(Search, { query: "Rain Fall" });
    await settleSearch();
    expect(backend.last("lyric_search")).toEqual({ query: "rain fall", limit: 12 });
    expect(screen.getByRole("heading", { name: "Lyrics" })).toBeInTheDocument();
    const row = screen.getByRole("button", { name: "Play Second Song from the line: the rain is falling today" });
    expect(row).toHaveTextContent("0:42");
    // A song no longer in the library isn't listed.
    expect(screen.queryByText("the rain is falling")).toBeNull();
  });

  test("clicking it plays the song with its album, from that line (once the song has started)", async () => {
    const backend = open([{ trackId: b.id, line: "the rain is falling today", time: 42.5 }]);
    app.status = makeStatus({ queue: [a.id], index: 0, position: 80, duration: 200, playing: true });
    render(Search, { query: "rain" });
    await settleSearch();
    await fireEvent.click(screen.getByRole("button", { name: /^Play Second Song from the line/ }));
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.last("play_tracks")).toEqual({ ids: [a.id, b.id], start: 1, picked: true });
    // Not sent at once: it would land on the song before.
    expect(backend.to("seek")).toHaveLength(0);
    // An update still about the song before: no seek yet.
    playerStatus({ queue: [a.id], index: 0, position: 80.25, duration: 200, playing: true });
    expect(backend.to("seek")).toHaveLength(0);
    // The new song has started: the seek, once.
    playerStatus({ queue: [a.id, b.id], index: 1, position: 0, duration: 200, playing: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.last("seek")).toEqual({ position: 42.5 });
    playerStatus({ queue: [a.id, b.id], index: 1, position: 0.25, duration: 200, playing: true });
    expect(backend.to("seek")).toHaveLength(1);
    // Play first, then the seek.
    const order = backend.calls.map((c) => c.cmd).filter((c) => c === "play_tracks" || c === "seek");
    expect(order).toEqual(["play_tracks", "seek"]);
  });

  test("the waiting seek gives up when the song never starts", async () => {
    const backend = open([{ trackId: b.id, line: "the rain is falling today", time: 42.5 }]);
    render(Search, { query: "rain" });
    await settleSearch();
    await fireEvent.click(screen.getByRole("button", { name: /^Play Second Song from the line/ }));
    await vi.advanceTimersByTimeAsync(10_000);
    playerStatus({ queue: [a.id, b.id], index: 1, position: 0, duration: 200, playing: true });
    expect(backend.to("seek")).toHaveLength(0);
  });

  test("the song's timing nudge applies; untimed lyrics play from the start", async () => {
    localStorage.setItem("cadence-lyric-offsets", JSON.stringify({ [b.id]: 1.5 }));
    const backend = open([
      { trackId: b.id, line: "the rain is falling today", time: 40 },
      { trackId: a.id, line: "the sky is blue", time: null },
    ]);
    render(Search, { query: "the" });
    await settleSearch();
    await fireEvent.click(screen.getByRole("button", { name: /^Play Second Song/ }));
    await vi.advanceTimersByTimeAsync(0);
    playerStatus({ queue: [a.id, b.id], index: 1, position: 0, duration: 200, playing: true });
    expect(backend.last("seek")).toEqual({ position: 41.5 });
    await fireEvent.click(screen.getByRole("button", { name: /^Play Test Song/ }));
    await vi.advanceTimersByTimeAsync(0);
    expect(backend.last("play_tracks")).toEqual({ ids: [a.id, b.id], start: 0, picked: true });
    expect(backend.to("seek")).toHaveLength(1);
  });

  test("while the index is first made: a note, then the results when it's ready", async () => {
    let ready = false;
    mockBackend({ lyric_search: () => ({ ready, hits: ready ? [{ trackId: a.id, line: "the sky is blue", time: 5 }] : [] }) });
    resetApp({ tracks: [a, b], albums: [makeAlbum([a, b])] });
    render(Search, { query: "sky blue" });
    await settleSearch();
    expect(screen.getByText(/Reading your saved lyrics once/)).toBeInTheDocument();
    ready = true;
    app.lyricIndexAt = Date.now(); // "lyric-index-ready"
    await settleSearch();
    expect(screen.getByRole("button", { name: /^Play Test Song from the line: the sky is blue/ })).toBeInTheDocument();
  });

  test("a new query never shows the last one's lines under it", async () => {
    let answer: (v: unknown) => void = () => {};
    let n = 0;
    mockBackend({
      lyric_search: () => (++n === 1 ? { ready: true, hits: [{ trackId: b.id, line: "the rain is falling today", time: 4 }] } : new Promise((r) => (answer = r))),
    });
    resetApp({ tracks: [a, b], albums: [makeAlbum([a, b])] });
    const view = render(Search, { query: "rain" });
    await settleSearch();
    expect(screen.getByText("the rain is falling today")).toBeInTheDocument();
    await view.rerender({ query: "sky" });
    // While typing (before the pause) the old lines are dimmed…
    expect(document.querySelector(".lyric-hits")).toHaveClass("stale");
    await settleSearch();
    // …and once the new query is asked, they're gone until its answer comes.
    expect(screen.queryByText("the rain is falling today")).toBeNull();
    answer({ ready: true, hits: [{ trackId: a.id, line: "the sky is blue", time: 5 }] });
    await vi.advanceTimersByTimeAsync(0);
    expect(screen.getByText("the sky is blue")).toBeInTheDocument();
  });

  test("malformed answers show nothing", async () => {
    open("nonsense");
    render(Search, { query: "rain" });
    await settleSearch();
    expect(screen.queryByRole("heading", { name: "Lyrics" })).toBeNull();
  });
});

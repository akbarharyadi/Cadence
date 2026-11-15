import { cleanup, render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockWindows } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import AppKeys from "#lib/components/AppKeys.svelte";
import PlaylistPicker from "#lib/components/PlaylistPicker.svelte";
import Page from "../routes/+page.svelte";
import type { Album, Track } from "#lib/api.js";
import { overlays } from "#lib/palette.svelte.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

// A small library: two albums by Kenshi Yonezu (one has the song "Lemon"), one by someone else.
const song = (title: string, artist: string, album: string, over: Partial<Track> = {}) =>
  makeTrack({ title, artist, albumArtist: artist, album, albumId: `${artist}|${album}`, ...over });

function library() {
  const tracks = [
    song("Lemon", "Kenshi Yonezu", "Bootleg", { trackNo: 1 }),
    song("Kick Back", "Kenshi Yonezu", "Bootleg", { trackNo: 2 }),
    song("Peace Sign", "Kenshi Yonezu", "Stray Sheep", { trackNo: 1 }),
    song("Halo", "Beyoncé", "I Am... Sasha Fierce"),
  ];
  const byAlbum = new Map<string, Track[]>();
  for (const t of tracks) byAlbum.set(t.albumId, [...(byAlbum.get(t.albumId) ?? []), t]);
  const albums: Album[] = [...byAlbum.values()].map((ts) => makeAlbum(ts));
  return { tracks, albums };
}

let backend: ReturnType<typeof mockBackend>;
let user: ReturnType<typeof userEvent.setup>;

beforeEach(() => {
  backend = mockBackend();
  user = userEvent.setup();
  overlays.paletteOpen = false;
  overlays.helpOpen = false;
});
afterEach(() => {
  overlays.paletteOpen = false;
  overlays.helpOpen = false;
});

const palette = () => screen.queryByRole("dialog", { name: "Command palette" });
const box = () => screen.getByRole("combobox", { name: /Search pages, actions/ });
const option = (name: RegExp | string) => screen.findByRole("option", { name });

async function open(lib = library(), status = null as ReturnType<typeof makeStatus> | null) {
  resetApp({ ...lib, status });
  const view = render(AppKeys);
  await user.keyboard("{Control>}k{/Control}");
  expect(palette()).toBeInTheDocument();
  return view;
}

describe("Command palette", () => {
  test("Ctrl+K opens it with the box focused; Ctrl+K again and Escape close it", async () => {
    await open();
    expect(box()).toHaveFocus();
    await user.keyboard("{Control>}k{/Control}");
    expect(palette()).not.toBeInTheDocument();

    await user.keyboard("{Control>}k{/Control}");
    expect(palette()).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(palette()).not.toBeInTheDocument();
  });

  test("nothing is built while it is closed", () => {
    resetApp(library());
    render(AppKeys);
    expect(palette()).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  test("an empty box shows recent pages and the most useful actions", async () => {
    resetApp(library());
    app.go({ name: "songs" });
    app.go({ name: "albums" });
    render(AppKeys);
    await user.keyboard("{Control>}k{/Control}");
    // Recent: the page left behind (Songs; Albums is where we are). Actions: playing, shuffle…
    expect(await option("Songs")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Play or pause/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Shuffle on or off/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Settings" })).not.toBeInTheDocument();
  });

  test("typing filters pages: 'sett' finds Settings, and Enter opens it", async () => {
    await open();
    await user.type(box(), "sett");
    const settings = await option("Settings");
    expect(settings).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("option", { name: "Albums" })).not.toBeInTheDocument();

    await user.keyboard("{Enter}");
    expect(app.view).toEqual({ name: "settings" });
    expect(palette()).not.toBeInTheDocument();
  });

  test("a playlist is a page too", async () => {
    resetApp({ ...library(), playlists: [{ id: "pl9", name: "Road trip", trackIds: [], smart: null, created: 1, updated: 1 }] });
    render(AppKeys);
    await user.keyboard("{Control>}k{/Control}");
    await user.type(box(), "road");
    await user.keyboard("{Enter}");
    expect(app.view).toEqual({ name: "playlist", id: "pl9" });
  });

  test("typing filters actions, shows their keys, and Enter runs the right one", async () => {
    await open();
    await user.type(box(), "shuffle");
    const action = await option(/Shuffle on or off/);
    expect(within(action).getByText("S")).toBeInTheDocument();

    await user.keyboard("{Enter}");
    await waitFor(() => expect(backend.to("set_shuffle")).toHaveLength(1));
    expect(backend.last("set_shuffle")).toEqual({ on: true });
    expect(palette()).not.toBeInTheDocument();
  });

  test("another action: 'rescan' runs the rescan", async () => {
    await open();
    await user.type(box(), "rescan");
    await option("Rescan library");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(backend.to("rescan")).toHaveLength(1));
  });

  test("the sleep timer's choices are actions", async () => {
    await open();
    await user.type(box(), "sleep 30");
    await option(/Sleep timer: In 30 minutes/);
    await user.keyboard("{Enter}");
    await waitFor(() => expect(backend.last("set_sleep_timer")).toEqual({ kind: "minutes", amount: 30 }));
  });

  test("library results: songs, albums and artists, each opened or played with Enter", async () => {
    await open();
    await user.type(box(), "yonezu");
    // The artist and her albums come up; the songs too (the artist's name is in them).
    const artist = await option(/^Kenshi Yonezu/);
    expect(screen.getByRole("option", { name: /^Bootleg/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /^Lemon/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Halo/ })).not.toBeInTheDocument();

    await user.hover(artist);
    await user.click(artist);
    expect(app.view).toEqual({ name: "artist", artist: "Kenshi Yonezu" });
  });

  test("Enter on an album opens it; Enter on a song plays it with the rest of its album", async () => {
    const lib = library();
    await open(lib);
    await user.type(box(), "stray");
    await option(/^Stray Sheep/);
    await user.keyboard("{Enter}");
    expect(app.view).toEqual({ name: "album", id: "Kenshi Yonezu|Stray Sheep" });

    // Again for a song: "kick" is the second song of Bootleg.
    await user.keyboard("{Control>}k{/Control}");
    await user.type(box(), "kick");
    await option(/^Kick Back/);
    await user.keyboard("{Enter}");
    await waitFor(() => expect(backend.to("play_tracks")).toHaveLength(1));
    const [lemon, kick] = lib.tracks;
    expect(backend.last("play_tracks")).toEqual({ ids: [lemon.id, kick.id], start: 1, picked: true });
  });

  test("the arrow keys move through the list, wrapping round, and the mouse picks too", async () => {
    await open();
    await user.type(box(), "s");
    await option(/^Sound/); // the filtered list, not the recent one
    const options = () => screen.getAllByRole("option");
    expect(options()[0]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}");
    expect(options()[1]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(options().at(-1)).toHaveAttribute("aria-selected", "true");
    await user.hover(options()[2]);
    expect(options()[2]).toHaveAttribute("aria-selected", "true");
  });

  test("nothing matching says so", async () => {
    await open();
    await user.type(box(), "zzzzqqq");
    expect(await screen.findByText(/Nothing matches/)).toBeInTheDocument();
  });

  test("on a library of 30,000 songs the results stay capped", async () => {
    const many = Array.from({ length: 30_000 }, (_, i) => song(`Track ${i}`, `Band ${i % 300}`, `Record ${i % 3000}`));
    resetApp({ tracks: many, albums: [] });
    render(AppKeys);
    await user.keyboard("{Control>}k{/Control}");
    await user.type(box(), "track");
    // At most 6 songs, though 30,000 match.
    const songs = () => screen.queryAllByRole("option").filter((o) => /^\s*Track \d+/.test(o.textContent ?? ""));
    await waitFor(() => expect(songs()).toHaveLength(6), { timeout: 4000 });
  });

  test("Escape closes only the palette when a dialog is open under it", async () => {
    resetApp({ ...library(), playlists: [] });
    app.picker = [makeTrack()];
    render(PlaylistPicker);
    render(AppKeys);
    expect(screen.getByRole("dialog", { name: /to a playlist/ })).toBeInTheDocument();

    await user.keyboard("{Control>}k{/Control}");
    expect(palette()).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(palette()).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: /to a playlist/ })).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(app.picker).toBeNull();
  });
});

describe("the window's shortcuts while the palette is open", () => {
  // The page unsubscribes from the webview when it goes: do that while the mocked backend is still there.
  afterEach(() => cleanup());

  // The real page, with its key handler (the backend answers nothing: the page shows its start-up
  // error, which doesn't matter here).
  async function page() {
    mockWindows("main");
    backend = mockBackend();
    resetApp({ ...library(), status: makeStatus({ queue: ["x"], index: 0 }) });
    render(Page);
    await settle();
  }

  test("Space plays and pauses with the palette closed, and does not while it is open", async () => {
    await page();
    (document.activeElement as HTMLElement | null)?.blur();
    await user.keyboard(" ");
    expect(backend.to("player").filter((c) => c.args.action === "toggle")).toHaveLength(1);

    await user.keyboard("{Control>}k{/Control}");
    expect(palette()).toBeInTheDocument();
    // Even with the keyboard on the page behind (a click on the overlay's text, say).
    (document.activeElement as HTMLElement | null)?.blur();
    await user.keyboard(" ");
    await user.keyboard("{Control>}{ArrowRight}{/Control}");
    await user.keyboard("{ArrowRight}");
    expect(backend.to("player").filter((c) => c.args.action === "toggle")).toHaveLength(1);
    expect(backend.to("player").filter((c) => c.args.action === "next")).toHaveLength(0);
    expect(backend.to("seek")).toHaveLength(0);
  });

  test("Space typed in the palette's box is a space, not a pause", async () => {
    await page();
    await user.keyboard("{Control>}k{/Control}");
    await user.type(box(), "new playlist");
    expect(box()).toHaveValue("new playlist");
    expect(backend.to("player")).toHaveLength(0);
  });
});

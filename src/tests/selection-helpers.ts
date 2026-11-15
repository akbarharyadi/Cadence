// Shared setup of the song-selection tests (track-selection*.test.ts): a six-song list, the list and the
// playlist page rendered with their menu, and short names for what the tests look at.
import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import ContextMenu from "#lib/components/ContextMenu.svelte";
import TrackList from "#lib/components/TrackList.svelte";
import PlaylistView from "#lib/views/PlaylistView.svelte";
import type { Playlist, Track } from "#lib/api.js";
import { makeAlbum, makePlaylist, makeTrack, mockBackend, resetApp } from "./helpers";

type Letter = "A" | "B" | "C" | "D" | "E" | "F";

/** Songs A–F, by title (`fx.tracks`, replaced per test by `setupSongs`), and their ids. */
export const fx = { tracks: [] as Track[], id: {} as Record<Letter, string> };

/** For `beforeEach`: six fresh songs. */
export function setupSongs() {
  fx.tracks = ["A", "B", "C", "D", "E", "F"].map((l) => makeTrack({ title: `Song ${l}` }));
  const [A, B, C, D, E, F] = fx.tracks.map((t) => t.id);
  fx.id = { A, B, C, D, E, F };
}

export function openList(list: Track[] = fx.tracks, props: Record<string, unknown> = {}) {
  const backend = mockBackend();
  resetApp({ tracks: fx.tracks, albums: [makeAlbum(fx.tracks)] });
  render(TrackList, { tracks: list, ...props });
  render(ContextMenu);
  return { backend, user: userEvent.setup() };
}
export function openPlaylist(trackIds: string[]) {
  const backend = mockBackend();
  const p = makePlaylist(trackIds);
  resetApp({ tracks: fx.tracks, albums: [makeAlbum(fx.tracks)], playlists: [p] });
  render(PlaylistView, { id: p.id });
  render(ContextMenu);
  return { backend, user: userEvent.setup() };
}

export const rows = () => screen.getAllByRole("option");
export const row = (title: string, nth = 0) => screen.getAllByRole("option", { name: new RegExp(`${title}\\b`) })[nth];
/** The titles of the selected rows, top to bottom. */
export const selected = () => rows().filter((r) => r.getAttribute("aria-selected") === "true").map((r) => r.querySelector(".title")!.textContent!.trim());
export const list = () => screen.getByRole("listbox");
export const key = (k: string, init: KeyboardEventInit = {}) => fireEvent.keyDown(list(), { key: k, ...init });
export const savedIds = (b: ReturnType<typeof mockBackend>) => (b.last("save_playlist")!.playlist as Playlist).trackIds;
export const menuLabels = () => screen.queryAllByRole("menuitem").map((m) => m.textContent!.trim());

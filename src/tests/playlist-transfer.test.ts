import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import PlaylistTransfer from "#lib/components/PlaylistTransfer.svelte";
import type { PlaylistImport } from "#lib/api.js";
import { startExport, startExportAll, startImport, transfer } from "#lib/playlistTransfer.svelte.js";
import { app } from "#lib/store.svelte.js";
import { makeLibrary, makePlaylist, mockBackend, resetApp, settle } from "./helpers";

const preview: PlaylistImport = {
  source: "C:\\Playlists\\Road.m3u8",
  name: "Road",
  format: "M3U",
  total: 5,
  matched: ["t3", "t1", "t3"],
  notFound: 1,
  ambiguous: 1,
  misses: [
    { artist: "Reol", title: "Sol", album: "", duration: 200, isrc: "", reason: "Not in your library", ambiguous: false },
    { artist: "A", title: "Song", album: "", duration: null, isrc: "", reason: "2 songs in your library fit equally well", ambiguous: true },
  ],
  moreMisses: 0,
  error: null,
};

beforeEach(() => {
  localStorage.clear();
  transfer.exporting = null;
  transfer.imports = [];
  transfer.choosing = null;
  transfer.reading = false;
});
afterEach(() => localStorage.clear());

async function openImport(extra: Record<string, () => unknown> = {}, previews: PlaylistImport[] = [preview]) {
  const backend = mockBackend({
    "plugin:dialog|open": () => previews.map((p) => p.source),
    playlist_import: () => previews,
    ...extra,
  });
  resetApp();
  render(PlaylistTransfer);
  const user = userEvent.setup();
  await startImport();
  await settle();
  return { backend, user, dialog: screen.getByRole("dialog", { name: "Import playlist" }) };
}

describe("Import playlist: the result is shown before anything is saved", () => {
  test("counts and the unmatched entries are listed, and nothing is saved by looking", async () => {
    const { backend, dialog } = await openImport();
    expect(backend.last("playlist_import")).toEqual({ paths: [preview.source] });
    const counts = within(dialog).getByRole("status");
    expect(counts.textContent).toMatch(/3\s*matched/);
    expect(counts.textContent).toMatch(/1\s*not found/);
    expect(counts.textContent).toMatch(/1\s*ambiguous/);
    expect((within(dialog).getByLabelText("Playlist name") as HTMLInputElement).value).toBe("Road");
    const list = within(dialog).getByRole("list", { name: "Entries not added" });
    expect(list.textContent).toContain("Reol – Sol");
    expect(list.textContent).toContain("Not in your library");
    expect(list.textContent).toContain("Ambiguous: 2 songs in your library fit equally well");
    expect(backend.to("save_playlist")).toHaveLength(0);
  });

  test("Create playlist saves the matched songs in the file's order, duplicates kept, under the chosen name", async () => {
    const { backend, user, dialog } = await openImport();
    const name = within(dialog).getByLabelText("Playlist name");
    await user.clear(name);
    await user.type(name, "Road trip 2");
    await user.click(within(dialog).getByRole("button", { name: "Create playlist" }));
    await settle();
    await settle();
    const saved = backend.last("save_playlist")?.playlist as { id: string; name: string; trackIds: string[]; smart: unknown };
    expect(saved).toMatchObject({ id: "", name: "Road trip 2", trackIds: ["t3", "t1", "t3"], smart: null });
    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Cancel saves nothing", async () => {
    const { backend, user, dialog } = await openImport();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await settle();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(backend.to("save_playlist")).toHaveLength(0);
  });

  test("a name that is taken is created as a numbered one by default, or the playlist's songs are replaced", async () => {
    const lib = makeLibrary();
    const mine = makePlaylist(["x"], { id: "pl9", name: "road" });
    const backend = mockBackend({ "plugin:dialog|open": () => [preview.source], playlist_import: () => [preview] });
    resetApp({ tracks: lib.tracks, albums: lib.albums, playlists: [mine] });
    render(PlaylistTransfer);
    const user = userEvent.setup();
    await startImport();
    await settle();
    let dialog = screen.getByRole("dialog", { name: "Import playlist" });
    expect(dialog.textContent).toMatch(/already have a playlist called/);
    await user.click(within(dialog).getByRole("button", { name: "Create playlist" }));
    await settle();
    await settle();
    expect(backend.last("save_playlist")?.playlist).toMatchObject({ id: "", name: "Road 2", trackIds: ["t3", "t1", "t3"] });

    transfer.imports = [preview];
    await settle();
    dialog = screen.getByRole("dialog", { name: "Import playlist" });
    await user.click(within(dialog).getByRole("radio", { name: /Replace its songs/ }));
    await user.click(within(dialog).getByRole("button", { name: "Create playlist" }));
    await settle();
    await settle();
    expect(backend.last("save_playlist")?.playlist).toMatchObject({ id: "pl9", name: "road", trackIds: ["t3", "t1", "t3"] });
  });

  test("Save the missing list asks where, then writes the file from the source", async () => {
    const { backend, user, dialog } = await openImport({
      "plugin:dialog|save": () => "C:\\Out\\Road - not in library.csv",
      playlist_missing_save: () => ({ path: "C:\\Out\\Road - not in library.csv", rows: 2 }),
    });
    await user.click(within(dialog).getByRole("button", { name: "Save the missing list…" }));
    await settle();
    await settle();
    expect(backend.last("playlist_missing_save")).toEqual({ source: preview.source, path: "C:\\Out\\Road - not in library.csv" });
    expect(within(dialog).getByText(/Saved 2 entries to/)).toBeInTheDocument();
    expect(backend.to("save_playlist")).toHaveLength(0);
  });

  test("a file that can't be read says why and creates nothing", async () => {
    const bad: PlaylistImport = { ...preview, matched: [], misses: [], notFound: 0, ambiguous: 0, total: 0, error: "“Road.m3u8” is 80 MB: too big to be a playlist (the limit is 64 MB)." };
    const { backend, dialog } = await openImport({}, [bad]);
    expect(within(dialog).getByRole("alert").textContent).toMatch(/too big/);
    expect(within(dialog).queryByRole("button", { name: "Create playlist" })).toBeNull();
    expect(backend.to("save_playlist")).toHaveLength(0);
  });

  test("a malformed answer doesn't break the page", async () => {
    const odd = { source: "x.csv", name: "", format: "CSV", total: 0, notFound: 0, ambiguous: 0, moreMisses: 0, error: null } as unknown as PlaylistImport;
    const { dialog } = await openImport({}, [odd]);
    expect(within(dialog).getByRole("button", { name: "Create playlist" })).toBeDisabled();
  });

  test("several files are asked about one after the other", async () => {
    const second: PlaylistImport = { ...preview, source: "C:\\Playlists\\Two.m3u8", name: "Two" };
    const { backend, user, dialog } = await openImport({}, [preview, second]);
    expect(backend.last("playlist_import")).toEqual({ paths: [preview.source, second.source] });
    await user.click(within(dialog).getByRole("button", { name: "Create playlist" }));
    await settle();
    await settle();
    const next = screen.getByRole("dialog", { name: "Import playlist" });
    expect((within(next).getByLabelText("Playlist name") as HTMLInputElement).value).toBe("Two");
  });
});

describe("Export…: the choices are passed through", () => {
  const setup = (opts?: object) => {
    const lib = makeLibrary(3);
    const pl = makePlaylist([lib.tracks[0].id, lib.tracks[2].id, "gone"], { name: "Road trip" });
    if (opts) localStorage.setItem("cadence-playlist-export", JSON.stringify(opts));
    const backend = mockBackend({
      "plugin:dialog|save": () => "C:\\Out\\Road trip.m3u8",
      "plugin:dialog|open": () => "C:\\Out",
      playlist_export: () => ({ path: "C:\\Out\\Road trip.m3u8", written: 2, skipped: 1, cue: 0 }),
      playlist_export_all: () => ({ folder: "C:\\Out", files: 1, written: 2, skipped: 1, cue: 0 }),
    });
    resetApp({ tracks: lib.tracks, albums: lib.albums, playlists: [pl] });
    render(PlaylistTransfer);
    return { backend, pl, lib, user: userEvent.setup() };
  };

  test("M3U8 is the default, and the options go to the command with the song ids", async () => {
    const { backend, pl, lib, user } = setup({ format: "m3u8", relative: false, forwardSlashes: false, crlf: false });
    startExport(pl);
    await settle();
    const dialog = screen.getByRole("dialog", { name: "Export playlist" });
    expect(within(dialog).getByRole("radio", { name: "M3U8" })).toHaveAttribute("aria-checked", "true");
    expect(within(dialog).getByText(/1 song isn’t in your library/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("radio", { name: "Relative to the file" }));
    await user.click(within(dialog).getByRole("checkbox", { name: /forward slashes/ }));
    await user.click(within(dialog).getByRole("checkbox", { name: /CRLF/ }));
    await user.click(within(dialog).getByRole("button", { name: "Export…" }));
    await settle();
    await settle();
    expect(backend.last("playlist_export")).toEqual({
      name: "Road trip",
      ids: [lib.tracks[0].id, lib.tracks[2].id, "gone"],
      path: "C:\\Out\\Road trip.m3u8",
      options: { format: "m3u8", relative: true, forwardSlashes: true, crlf: true },
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(String(app.toast ?? "")).toMatch(/Saved “Road trip” \(2 songs\)/);
    // The choices are remembered for next time.
    expect(JSON.parse(localStorage.getItem("cadence-playlist-export") ?? "{}")).toMatchObject({ relative: true, crlf: true });
  });

  test("CSV drops the line-ending choice and asks for a .csv file", async () => {
    const { backend, pl, user } = setup({ format: "m3u8", relative: false, forwardSlashes: false, crlf: false });
    startExport(pl);
    await settle();
    const dialog = screen.getByRole("dialog", { name: "Export playlist" });
    await user.click(within(dialog).getByRole("radio", { name: "CSV" }));
    expect(within(dialog).queryByRole("checkbox", { name: /CRLF/ })).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Export…" }));
    await settle();
    await settle();
    expect(JSON.stringify(backend.last("plugin:dialog|save"))).toMatch(/Road trip\.csv/);
    expect((backend.last("playlist_export")?.options as { format: string }).format).toBe("csv");
  });

  test("Cancel exports nothing", async () => {
    const { backend, pl, user } = setup();
    startExport(pl);
    await settle();
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
    await settle();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(backend.to("playlist_export")).toHaveLength(0);
  });

  test("Export all playlists asks for a folder and sends every playlist's songs", async () => {
    const { backend, lib, user } = setup({ format: "csv", relative: false, forwardSlashes: true, crlf: false });
    startExportAll();
    await settle();
    const dialog = screen.getByRole("dialog", { name: "Export all playlists" });
    await user.click(within(dialog).getByRole("button", { name: "Choose folder…" }));
    await settle();
    await settle();
    expect(backend.last("playlist_export_all")).toEqual({
      items: [{ name: "Road trip", ids: [lib.tracks[0].id, lib.tracks[2].id, "gone"] }],
      folder: "C:\\Out",
      options: { format: "csv", relative: false, forwardSlashes: true, crlf: false },
    });
  });

  test("the dialog says when songs are tracks of one file (CUE)", async () => {
    const lib = makeLibrary(2);
    lib.tracks[0].cue = { start: 0, end: 100 };
    const pl = makePlaylist([lib.tracks[0].id, lib.tracks[1].id]);
    mockBackend();
    resetApp({ tracks: lib.tracks, albums: lib.albums, playlists: [pl] });
    render(PlaylistTransfer);
    startExport(pl);
    await settle();
    expect(screen.getByRole("dialog").textContent).toMatch(/CUE sheet/);
  });
});


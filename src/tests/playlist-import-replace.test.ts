import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test } from "vitest";
import PlaylistTransfer from "#lib/components/PlaylistTransfer.svelte";
import type { PlaylistImport } from "#lib/api.js";
import { startImport, transfer } from "#lib/playlistTransfer.svelte.js";
import { app } from "#lib/store.svelte.js";
import { makeLibrary, makePlaylist, mockBackend, resetApp, settle } from "./helpers";

// A file of which only one entry is in the library, named like a playlist of 12 songs.
const preview: PlaylistImport = {
  source: "C:\\Playlists\\Road.m3u8",
  name: "Road",
  format: "M3U",
  total: 300,
  matched: ["t1"],
  notFound: 299,
  ambiguous: 0,
  misses: [{ artist: "Invented Band", title: "Made Up Song", album: "", duration: 200, isrc: "", reason: "Not in your library", ambiguous: false }],
  moreMisses: 298,
  error: null,
};

const twelve = Array.from({ length: 12 }, (_, i) => `old${i}`);

beforeEach(() => {
  localStorage.clear();
  transfer.exporting = null;
  transfer.imports = [];
  transfer.choosing = null;
  transfer.reading = false;
});

async function openReplacing(previews: PlaylistImport[] = [preview], songs = twelve) {
  const lib = makeLibrary();
  const mine = makePlaylist(songs, { id: "pl9", name: "road" });
  const backend = mockBackend({ "plugin:dialog|open": () => previews.map((p) => p.source), playlist_import: () => previews });
  resetApp({ tracks: lib.tracks, albums: lib.albums, playlists: [mine] });
  render(PlaylistTransfer);
  const user = userEvent.setup();
  await startImport();
  await settle();
  const dialog = screen.getByRole("dialog", { name: "Import playlist" });
  await user.click(within(dialog).getByRole("radio", { name: /Replace its songs/ }));
  return { backend, user, dialog };
}

describe("Import playlist: replacing a playlist's songs with far fewer", () => {
  test("asks first, with both counts, and saves nothing until confirmed", async () => {
    const { backend, user, dialog } = await openReplacing();
    await user.click(within(dialog).getByRole("button", { name: "Create playlist" }));
    await settle();
    const alert = within(dialog).getByRole("alert");
    expect(alert.textContent).toMatch(/leaves “road” with 1 song/);
    expect(alert.textContent).toMatch(/it has 12 songs now/);
    expect(backend.to("save_playlist")).toHaveLength(0);
    // Choosing to create a new one instead asks nothing.
    await user.click(within(dialog).getByRole("radio", { name: /Create “Road 2”/ }));
    expect(within(dialog).queryByRole("alert")).toBeNull();
    await user.click(within(dialog).getByRole("radio", { name: /Replace its songs/ }));
    await user.click(within(dialog).getByRole("button", { name: "Create playlist" }));
    await user.click(within(dialog).getByRole("button", { name: "Replace anyway" }));
    await settle();
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(backend.last("save_playlist")?.playlist).toMatchObject({ id: "pl9", trackIds: ["t1"] });
  });

  test("Undo in the toast saves the songs it had before", async () => {
    const { backend, user, dialog } = await openReplacing();
    await user.click(within(dialog).getByRole("button", { name: "Create playlist" }));
    await user.click(within(dialog).getByRole("button", { name: "Replace anyway" }));
    await settle();
    await settle();
    expect(app.toast).toMatch(/Replaced the songs of “road” with 1 song/);
    expect(app.toastAction?.label).toBe("Undo");
    app.toastAction?.run();
    await settle();
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(2);
    expect(backend.last("save_playlist")?.playlist).toMatchObject({ id: "pl9", name: "road", trackIds: twelve });
    expect(app.toast).toMatch(/has its 12 songs back/);
  });

  test("a replacement that keeps most of the songs isn't asked about (and can still be undone)", async () => {
    const most: PlaylistImport = { ...preview, matched: ["t1", "t2", "t3"], notFound: 0, total: 3, misses: [], moreMisses: 0 };
    const { backend, user, dialog } = await openReplacing([most], ["old0", "old1", "old2", "old3"]);
    await user.click(within(dialog).getByRole("button", { name: "Create playlist" }));
    await settle();
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(app.toastAction?.label).toBe("Undo");
  });
});

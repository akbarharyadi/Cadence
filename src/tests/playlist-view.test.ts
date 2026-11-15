import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test } from "vitest";
import ContextMenu from "#lib/components/ContextMenu.svelte";
import PlaylistView from "#lib/views/PlaylistView.svelte";
import type { Playlist, Track } from "#lib/api.js";
import { makeAlbum, makePlaylist, makeTrack, mockBackend, resetApp, settle } from "./helpers";

// Library: songs A–E. The playlist also lists X, a song the library doesn't have (its file may come
// back), between B and C: so a row's number is not its position in the saved list.
let ids: Record<"A" | "B" | "C" | "D" | "E" | "X", string>;
let tracks: Track[];

function open(playlist: Playlist = makePlaylist([ids.A, ids.B, ids.X, ids.C, ids.D, ids.E])) {
  const backend = mockBackend();
  resetApp({ tracks, albums: [makeAlbum(tracks)], playlists: [playlist] });
  render(PlaylistView, { id: playlist.id });
  render(ContextMenu); // the right-click menu lives in the app shell, not in the view
  return { backend, user: userEvent.setup() };
}
const row = (title: string) => screen.getByRole("option", { name: new RegExp(`${title}\\b`) });
const savedIds = (b: ReturnType<typeof mockBackend>) => (b.last("save_playlist")!.playlist as Playlist).trackIds;

beforeEach(() => {
  tracks = ["A", "B", "C", "D", "E"].map((l) => makeTrack({ title: `Song ${l}` }));
  const [A, B, C, D, E] = tracks.map((t) => t.id);
  ids = { A, B, C, D, E, X: "gone-from-library" };
});

describe("PlaylistView with a song the library doesn't have", () => {
  test("the header says one song is missing, and the rows are the five that exist", () => {
    open();
    expect(screen.getByText(/1 song missing/)).toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(5);
  });

  test("removing the row that shows C saves [A, B, X, D, E] (X stays, nothing else goes)", async () => {
    const { backend, user } = open();

    await user.pointer({ keys: "[MouseRight]", target: row("Song C") });
    await user.click(await screen.findByRole("menuitem", { name: "Remove from this playlist" }));
    await settle();

    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(savedIds(backend)).toEqual([ids.A, ids.B, ids.X, ids.D, ids.E]);
  });

  test("dragging D to the top saves [D, A, B, X, C, E]", async () => {
    const { backend } = open();
    // jsdom has no drag and drop: the rows get the same events a browser sends, with a minimal
    // dataTransfer that carries the row type the list checks for.
    const dataTransfer = { types: ["application/x-cadence-row"], setData() {}, effectAllowed: "" };

    await fireEvent.dragStart(row("Song D"), { dataTransfer });
    await fireEvent.dragOver(row("Song A"), { dataTransfer });
    await fireEvent.drop(row("Song A"), { dataTransfer });
    await settle();

    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(savedIds(backend)).toEqual([ids.D, ids.A, ids.B, ids.X, ids.C, ids.E]);
  });

  test("a drop that doesn't carry Cadence's row type (a file from outside) saves nothing", async () => {
    const { backend } = open();
    const foreign = { types: ["Files"], setData() {}, effectAllowed: "" };
    await fireEvent.dragStart(row("Song D"), { dataTransfer: { types: ["application/x-cadence-row"], setData() {}, effectAllowed: "" } });
    await fireEvent.drop(row("Song A"), { dataTransfer: foreign });
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(0);
  });
});

describe("PlaylistView rename", () => {
  const startRename = async (user: ReturnType<typeof userEvent.setup>) => {
    await user.dblClick(screen.getByRole("button", { name: "Road trip" }));
    return screen.getByRole("textbox", { name: "Playlist name" });
  };

  test("Escape while renaming does not save (not even from the blur that follows)", async () => {
    const { backend, user } = open();
    const input = await startRename(user);
    await user.clear(input);
    await user.type(input, "Weekend");

    // Escape, then (before the page updates) the blur the browser sends as the box goes away.
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    input.dispatchEvent(new FocusEvent("blur"));
    await settle();

    expect(backend.to("save_playlist")).toHaveLength(0);
    expect(screen.queryByRole("textbox", { name: "Playlist name" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Road trip" })).toBeInTheDocument();
  });

  test("Enter does save the new name (so the test above isn't passing for the wrong reason)", async () => {
    const { backend, user } = open();
    const input = await startRename(user);
    await user.clear(input);
    await user.type(input, "Weekend{Enter}");
    await settle();

    expect(backend.to("save_playlist")).toHaveLength(1);
    expect((backend.last("save_playlist")!.playlist as Playlist).name).toBe("Weekend");
  });
});

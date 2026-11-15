import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import NeedsAttention from "#lib/views/NeedsAttention.svelte";
import type { DupGroup, Removed } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeTrack, mockBackend, resetApp, settle } from "./helpers";

// One certain duplicate (a "sure" copy: the kind that is ticked for you where Undo can bring the
// file back): a FLAC to keep and a lossy copy of the same song to remove.
const keep = makeTrack({ title: "Lemon", artist: "Kenshi Yonezu", albumArtist: "Kenshi Yonezu", album: "Bootleg", albumId: "b" });
const copy = makeTrack({ title: "Lemon", artist: "Kenshi Yonezu", albumArtist: "Kenshi Yonezu", album: "Bootleg", albumId: "b", codec: "MP3", bitDepth: null, size: 5_000_000 });
const group: DupGroup = {
  kind: "lossy",
  songs: [
    { id: keep.id, keep: true },
    { id: copy.id, keep: false },
  ],
  wasted: 5_000_000,
  sure: true,
};
const removed: Removed = { deleted: 1, freed: 5_000_000, skipped: [], change: null, recycled: false };

function open() {
  const backend = mockBackend({
    artist_pairs: () => ({ pairs: [], unchecked: 0, running: false }),
    fix_all_found: () => [],
    covers_list: () => ({ found: [], missed: [] }),
    recent_changes: () => [],
    health_remove: () => removed,
  });
  resetApp({ tracks: [keep, copy], albums: [makeAlbum([keep, copy], { art: "x.jpg" })] });
  app.duplicates = [group];
  render(NeedsAttention);
  return { backend, user: userEvent.setup() };
}

/** The platform is read once, when the page opens: set it first. */
const asAndroid = () => ((window as unknown as { CadenceAndroid: unknown }).CadenceAndroid = { update() {}, hasMusicPermission: () => true });
const asMac = () => vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15");

const tickbox = () => screen.getByRole("checkbox", { name: "Remove the copy of Lemon" });

describe("Needs Attention: removing extra copies where Undo can't bring them back", () => {
  test.each([
    ["Android (deleted for good)", asAndroid, /Delete 0 copies/, /^Delete copy$/],
    ["macOS (Trash, no Undo)", asMac, /Move 0 copies to the Trash/, /^Move copy$/],
  ])("%s: nothing is pre-ticked, and removing asks first", async (_platform, setPlatform, idleButton, confirmButton) => {
    setPlatform();
    const { backend, user } = open();
    await settle();

    // A sure copy, and still not ticked.
    expect(tickbox()).not.toBeChecked();
    expect(screen.getByRole("button", { name: idleButton })).toBeDisabled();

    await user.click(tickbox());
    const batch = screen.getByRole("button", { name: /1 cop(y|ies)/ });
    expect(batch).toBeEnabled();
    await user.click(batch);

    // A question first; nothing was removed yet.
    const dialog = screen.getByRole("dialog");
    expect(dialog).toBeInTheDocument();
    expect(backend.to("health_remove")).toHaveLength(0);

    // Cancelling removes nothing either.
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(backend.to("health_remove")).toHaveLength(0);

    // Asked again and confirmed: now it goes ahead, with just the copy (not the song that stays).
    await user.click(screen.getByRole("button", { name: /1 cop(y|ies)/ }));
    await user.click(screen.getByRole("button", { name: confirmButton }));
    await settle();
    expect(backend.to("health_remove")).toHaveLength(1);
    expect(backend.last("health_remove")).toEqual({ ids: [copy.id] });
  });

  test("where Undo works (Windows, Linux) the sure copy is ticked and removal needs no question", async () => {
    // The control for the tests above: the same screen on a platform with a Recycle Bin.
    const { backend, user } = open();
    await settle();

    expect(tickbox()).toBeChecked();
    await user.click(screen.getByRole("button", { name: /Move 1 copy to the Recycle Bin/ }));
    await settle();

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(backend.last("health_remove")).toEqual({ ids: [copy.id] });
  });
});

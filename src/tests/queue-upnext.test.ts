import { fireEvent, render, screen, within } from "@testing-library/svelte";
import { beforeEach, describe, expect, test } from "vitest";
import ContextMenu from "#lib/components/ContextMenu.svelte";
import QueuePanel from "#lib/components/QueuePanel.svelte";
import { app } from "#lib/store.svelte.js";
import type { Status } from "#lib/api.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

let backend: ReturnType<typeof mockBackend>;
beforeEach(() => {
  backend = mockBackend();
});

/** A queue of `n` songs, the first playing. */
function queueOf(n: number, over: Partial<Status> = {}) {
  const tracks = Array.from({ length: n }, (_, i) => makeTrack({ title: `Song ${String.fromCharCode(65 + i)}` }));
  resetApp({ tracks, albums: [makeAlbum(tracks)], status: makeStatus({ queue: tracks.map((t) => t.id), index: 0, duration: 200, playing: true, ...over }) });
  return tracks;
}

const row = (title: string) => screen.getByText(title).closest(".item") as HTMLElement;
const labels = () => (app.menu?.items ?? []).map((i) => i.label);
const pick = (label: string) => {
  const item = app.menu?.items.find((i) => i.label === label);
  if (!item) throw new Error(`no "${label}" in ${labels().join(", ")}`);
  item.action();
};

describe("Up Next row menu", () => {
  test("a waiting song's menu has what a queue row needs, and each does it", async () => {
    const tracks = queueOf(5);
    render(QueuePanel);
    await settle();

    await fireEvent.contextMenu(row("Song D"));
    expect(labels()).toEqual([
      "Play Now",
      "Play Next",
      "Remove from Up Next",
      "Stop After This Song",
      "Go to Album",
      "Go to Artist",
      "Add to Playlist…",
      "Add to Favorites",
    ]);

    pick("Play Now");
    pick("Play Next");
    pick("Remove from Up Next");
    pick("Stop After This Song");
    await settle();
    expect(backend.last("jump")).toEqual({ index: 3 });
    expect(backend.last("move_in_queue")).toEqual({ from: 3, to: 1 });
    expect(backend.last("remove_from_queue")).toEqual({ index: 3 });
    expect(backend.last("stop_after_song")).toEqual({ index: 3 });

    pick("Add to Playlist…");
    expect(app.picker?.map((t) => t.id)).toEqual([tracks[3].id]);
    pick("Add to Favorites");
    await settle();
    expect(backend.last("set_loved")).toEqual({ ids: [tracks[3].id], loved: true });
    pick("Go to Album");
    expect(app.view).toEqual({ name: "album", id: tracks[3].albumId });
  });

  test("the next song has no Play Next, and the playing song only what fits it", async () => {
    queueOf(4);
    render(QueuePanel);
    await settle();
    await fireEvent.contextMenu(row("Song B"));
    expect(labels()).not.toContain("Play Next");
    expect(labels()).toContain("Play Now");

    await fireEvent.contextMenu(row("Song A"));
    expect(labels()).not.toContain("Play Now");
    expect(labels()).not.toContain("Remove from Up Next");
    expect(labels()).toContain("Stop After This Song");
    pick("Stop After This Song");
    await settle();
    expect(backend.last("stop_after_song")).toEqual({ index: 0 });
  });

  test("the menu opens from the keyboard too, and shows in the context menu", async () => {
    queueOf(3);
    render(QueuePanel);
    render(ContextMenu);
    await settle();
    await fireEvent.keyDown(row("Song C"), { key: "ContextMenu" });
    await settle();
    const menu = screen.getByRole("menu");
    expect(within(menu).getByText("Stop After This Song")).toBeInTheDocument();
  });
});

describe("Stop after this song", () => {
  test("the marked row says so, and its menu offers to undo it", async () => {
    queueOf(5, { stopMark: 2 });
    render(QueuePanel);
    await settle();
    expect(within(row("Song C")).getByText("Stop after")).toBeInTheDocument();
    expect(screen.getAllByText("Stop after")).toHaveLength(1);

    await fireEvent.contextMenu(row("Song C"));
    pick("Don’t Stop After This Song");
    await settle();
    expect(backend.last("stop_after_song")).toEqual({ index: null });
  });

  test("the marker follows the player's place for it", async () => {
    queueOf(5, { stopMark: 0 });
    render(QueuePanel);
    await settle();
    expect(within(row("Song A")).getByText("Stop after")).toBeInTheDocument();
    app.status = makeStatus({ ...app.status!, stopMark: 4 });
    await settle();
    expect(within(row("Song E")).getByText("Stop after")).toBeInTheDocument();
    app.status = makeStatus({ ...app.status!, stopMark: null });
    await settle();
    expect(screen.queryByText("Stop after")).not.toBeInTheDocument();
  });
});

describe("Autoplay in Up Next", () => {
  test("the songs Autoplay added are under their own heading, cleared in one go", async () => {
    queueOf(6, { autoplayFrom: 3 });
    render(QueuePanel);
    await settle();
    const heading = screen.getByText(/Autoplay/);
    // Above Song D, below Song C.
    const order = [...document.querySelectorAll(".scroll > *")];
    const at = (el: Element) => order.indexOf(el);
    expect(at(heading.closest(".label")!)).toBeGreaterThan(at(row("Song C")));
    expect(at(heading.closest(".label")!)).toBeLessThan(at(row("Song D")));

    await fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    await settle();
    expect(backend.to("clear_autoplay")).toHaveLength(1);
  });

  test("no heading without Autoplay's songs, nor from an older player that doesn't say", async () => {
    queueOf(4, { autoplayFrom: null });
    render(QueuePanel);
    await settle();
    expect(screen.queryByText(/Autoplay/)).not.toBeInTheDocument();
    const s = { ...app.status! } as Partial<Status>;
    delete s.autoplayFrom;
    delete s.stopMark;
    app.status = s as Status;
    await settle();
    expect(screen.queryByText(/Autoplay/)).not.toBeInTheDocument();
    expect(screen.getByText("Next, 3 songs")).toBeInTheDocument();
  });

  test("an album shuffle says so", async () => {
    queueOf(4, { albumShuffle: true });
    render(QueuePanel);
    await settle();
    expect(screen.getByText(/shuffling albums/)).toBeInTheDocument();
  });
});

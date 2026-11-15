import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { flushSync } from "svelte";
import { beforeEach, describe, expect, test } from "vitest";
import ContextMenu from "#lib/components/ContextMenu.svelte";
import TrackList from "#lib/components/TrackList.svelte";
import PlaylistView from "#lib/views/PlaylistView.svelte";
import type { Playlist, Track } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makePlaylist, makeTrack, mockBackend, resetApp, settle } from "./helpers";

// Songs A–F, by title; `X` is an id a playlist may list that the library doesn't have.
let tracks: Track[];
let id: Record<"A" | "B" | "C" | "D" | "E" | "F", string>;

beforeEach(() => {
  tracks = ["A", "B", "C", "D", "E", "F"].map((l) => makeTrack({ title: `Song ${l}` }));
  const [A, B, C, D, E, F] = tracks.map((t) => t.id);
  id = { A, B, C, D, E, F };
});

function openList(list: Track[] = tracks, props: Record<string, unknown> = {}) {
  const backend = mockBackend();
  resetApp({ tracks, albums: [makeAlbum(tracks)] });
  render(TrackList, { tracks: list, ...props });
  render(ContextMenu);
  return { backend, user: userEvent.setup() };
}
function openPlaylist(trackIds: string[]) {
  const backend = mockBackend();
  const p = makePlaylist(trackIds);
  resetApp({ tracks, albums: [makeAlbum(tracks)], playlists: [p] });
  render(PlaylistView, { id: p.id });
  render(ContextMenu);
  return { backend, user: userEvent.setup() };
}

const rows = () => screen.getAllByRole("option");
const row = (title: string, nth = 0) => screen.getAllByRole("option", { name: new RegExp(`${title}\\b`) })[nth];
/** The titles of the selected rows, top to bottom. */
const selected = () => rows().filter((r) => r.getAttribute("aria-selected") === "true").map((r) => r.querySelector(".title")!.textContent!.trim());
const list = () => screen.getByRole("listbox");
const key = (k: string, init: KeyboardEventInit = {}) => fireEvent.keyDown(list(), { key: k, ...init });
const savedIds = (b: ReturnType<typeof mockBackend>) => (b.last("save_playlist")!.playlist as Playlist).trackIds;
const menuLabels = () => screen.queryAllByRole("menuitem").map((m) => m.textContent!.trim());

describe("Selecting with the mouse", () => {
  test("click selects one; Ctrl+click adds and removes; Shift+click selects the range from the anchor", async () => {
    const { user } = openList();
    await user.click(row("Song B"));
    expect(selected()).toEqual(["Song B"]);

    await user.keyboard("{Control>}");
    await user.click(row("Song D"));
    await user.click(row("Song F"));
    await user.click(row("Song B")); // Ctrl+click on a selected row takes it out
    await user.keyboard("{/Control}");
    expect(selected()).toEqual(["Song D", "Song F"]);

    // The anchor is the last Ctrl+clicked row (B): Shift+click on E selects B..E, replacing the rest.
    await user.keyboard("{Shift>}");
    await user.click(row("Song E"));
    await user.keyboard("{/Shift}");
    expect(selected()).toEqual(["Song B", "Song C", "Song D", "Song E"]);

    // A plain click starts over.
    await user.click(row("Song A"));
    expect(selected()).toEqual(["Song A"]);
  });

  test("Shift+click upwards from the anchor works too", async () => {
    const { user } = openList();
    await user.click(row("Song E"));
    await user.keyboard("{Shift>}");
    await user.click(row("Song B"));
    await user.keyboard("{/Shift}");
    expect(selected()).toEqual(["Song B", "Song C", "Song D", "Song E"]);
  });

  test("the list is one listbox, multi-selectable, with one tab stop", () => {
    openList();
    expect(list()).toHaveAttribute("aria-multiselectable", "true");
    expect(list()).toHaveAttribute("tabindex", "0");
    for (const r of rows()) expect(r).not.toHaveAttribute("tabindex");
  });

  test("the bar appears for two or more songs, and its × clears them", async () => {
    const { user } = openList();
    await user.click(row("Song A"));
    expect(screen.queryByRole("toolbar", { name: "Selected songs" })).not.toBeInTheDocument();
    await user.keyboard("{Shift>}");
    await user.click(row("Song C"));
    await user.keyboard("{/Shift}");
    const bar = screen.getByRole("toolbar", { name: "Selected songs" });
    expect(within(bar).getByText("3 selected")).toBeInTheDocument();
    await user.click(within(bar).getByRole("button", { name: "Clear selection" }));
    expect(selected()).toEqual([]);
    expect(screen.queryByRole("toolbar", { name: "Selected songs" })).not.toBeInTheDocument();
  });
});

describe("Selecting with the keyboard", () => {
  test("Ctrl+A selects every song in the list", () => {
    openList();
    list().focus();
    key("a", { ctrlKey: true });
    expect(selected()).toEqual(tracks.map((t) => t.title));
  });

  test("Up/Down move the active row and select it; with Shift they extend from the anchor", async () => {
    const { user } = openList();
    await user.click(row("Song B"));
    key("ArrowDown");
    expect(selected()).toEqual(["Song C"]);
    expect(list().getAttribute("aria-activedescendant")).toBe(row("Song C").id);

    key("ArrowDown", { shiftKey: true });
    key("ArrowDown", { shiftKey: true });
    expect(selected()).toEqual(["Song C", "Song D", "Song E"]);
    key("ArrowUp", { shiftKey: true });
    expect(selected()).toEqual(["Song C", "Song D"]);
    // Above the anchor: the range turns around it.
    key("ArrowUp", { shiftKey: true });
    key("ArrowUp", { shiftKey: true });
    expect(selected()).toEqual(["Song B", "Song C"]);

    key("ArrowUp");
    expect(selected()).toEqual(["Song A"]);
    key("ArrowUp"); // stays at the top
    expect(selected()).toEqual(["Song A"]);
    key("End");
    expect(selected()).toEqual(["Song F"]);
    key("Home", { shiftKey: true });
    expect(selected()).toEqual(tracks.map((t) => t.title));
  });

  test("Enter plays the active song (like double-click)", async () => {
    const { backend, user } = openList();
    await user.click(row("Song C"));
    key("Enter");
    await settle();
    expect(backend.last("play_tracks")).toEqual({ ids: tracks.map((t) => t.id), start: 2, picked: true });
  });

  test("typing jumps to the next song whose title starts with it", () => {
    const list3 = [makeTrack({ title: "Alpha" }), makeTrack({ title: "Bravo" }), makeTrack({ title: "Banana" }), makeTrack({ title: "Charlie" })];
    tracks = list3;
    openList(list3);
    list().focus();
    key("b");
    expect(selected()).toEqual(["Bravo"]);
    key("b"); // the same letter again: the next one
    expect(selected()).toEqual(["Banana"]);
    key("b"); // and round again
    expect(selected()).toEqual(["Bravo"]);
  });

  test("type-ahead matches what is typed so far, and starts over after a short pause", async () => {
    const list3 = [makeTrack({ title: "Bravo" }), makeTrack({ title: "Banana" }), makeTrack({ title: "Charlie" })];
    tracks = list3;
    openList(list3);
    list().focus();
    key("b");
    key("a");
    expect(selected()).toEqual(["Banana"]);
    await new Promise((r) => setTimeout(r, 900));
    key("c");
    expect(selected()).toEqual(["Charlie"]);
  });

  test("Escape clears the selection and is used up (no Back, nothing behind it sees it)", async () => {
    const { user } = openList();
    app.history = [{ name: "albums" }];
    app.view = { name: "songs" };
    let seenByWindow: boolean | null = null;
    const spy = (e: KeyboardEvent) => (seenByWindow = e.defaultPrevented);
    window.addEventListener("keydown", spy);
    try {
      await user.click(row("Song A"));
      await user.keyboard("{Shift>}");
      await user.click(row("Song C"));
      await user.keyboard("{/Shift}");
      expect(selected()).toHaveLength(3);

      seenByWindow = null;
      key("Escape");
      expect(selected()).toEqual([]);
      expect(app.view).toEqual({ name: "songs" });
      expect(app.history).toHaveLength(1);
      // Stopped at the list: the window's Escape handlers (dialogs, Now Playing) never get it.
      expect(seenByWindow).toBeNull();
    } finally {
      window.removeEventListener("keydown", spy);
    }
  });

  test("Space without a modifier is left alone (it stays the app's play / pause)", async () => {
    const { user } = openList();
    await user.click(row("Song A"));
    const ev = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    list().dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(false);
    expect(selected()).toEqual(["Song A"]);
  });
});

describe("The right-click menu", () => {
  const selectBAndD = async (user: ReturnType<typeof userEvent.setup>) => {
    // D first, then B: the menu's actions still get them in list order.
    await user.click(row("Song D"));
    await user.keyboard("{Control>}");
    await user.click(row("Song B"));
    await user.keyboard("{/Control}");
  };

  test("on a row in the selection: the menu is for the whole selection, without single-song entries", async () => {
    const { user } = openList();
    await selectBAndD(user);
    await user.pointer({ keys: "[MouseRight]", target: row("Song D") });
    expect(selected()).toEqual(["Song B", "Song D"]);
    expect(menuLabels()).toEqual(["Play 2 Songs", "Play Next", "Add to Queue", "Add to Playlist…", "Add to Favorites", "Edit Info…", "Skip These Songs"]);
    for (const single of ["Go to Album", "Go to Artist", "Start Sound Radio", "Songs That Sound Like This", "Fix Song Info…", "Change Cover…"]) {
      expect(menuLabels()).not.toContain(single);
    }
    expect(within(screen.getByRole("menu")).getAllByRole("group", { name: "Rating" }).length).toBeGreaterThan(0);
  });

  test("on a row outside the selection: just that row is selected, and its own menu opens", async () => {
    const { user } = openList();
    await selectBAndD(user);
    await user.pointer({ keys: "[MouseRight]", target: row("Song F") });
    expect(selected()).toEqual(["Song F"]);
    expect(menuLabels()).toContain("Go to Album");
    expect(menuLabels()[0]).toBe("Play");
  });

  test("Add to Queue and Add to Playlist get every selected song, in list order", async () => {
    const { backend, user } = openList();
    await selectBAndD(user);
    await user.pointer({ keys: "[MouseRight]", target: row("Song B") });
    await user.click(screen.getByRole("menuitem", { name: "Add to Queue" }));
    await settle();
    expect(backend.last("enqueue")).toEqual({ ids: [id.B, id.D], next: false });

    await user.pointer({ keys: "[MouseRight]", target: row("Song D") });
    await user.click(screen.getByRole("menuitem", { name: "Add to Playlist…" }));
    expect(app.picker?.map((t) => t.id)).toEqual([id.B, id.D]);
  });

  test("Edit Info… opens the tag editor for all of them, and is hidden when a CUE-sheet song is selected", async () => {
    tracks[3] = { ...tracks[3], cue: { start: 0, end: 100 } }; // Song D
    const { user } = openList();
    await user.click(row("Song A"));
    await user.keyboard("{Shift>}");
    await user.click(row("Song C"));
    await user.keyboard("{/Shift}");
    await user.pointer({ keys: "[MouseRight]", target: row("Song B") });
    await user.click(screen.getByRole("menuitem", { name: "Edit Info…" }));
    expect(app.tagEditing?.map((t) => t.id)).toEqual([id.A, id.B, id.C]);

    await user.keyboard("{Shift>}");
    await user.click(row("Song D"));
    await user.keyboard("{/Shift}");
    await user.pointer({ keys: "[MouseRight]", target: row("Song D") });
    expect(menuLabels()).not.toContain("Edit Info…");
  });
});

describe("In a playlist", () => {
  test("a song listed twice: only the selected one of its two rows is removed", async () => {
    const { backend, user } = openPlaylist([id.A, id.B, id.A, id.C]);
    await user.click(row("Song A", 1)); // the second A
    key("Delete");
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(savedIds(backend)).toEqual([id.A, id.B, id.C]);
  });

  test("Delete removes exactly the selected rows, by their places (a song missing from the library stays)", async () => {
    const X = "gone-from-library";
    const { backend, user } = openPlaylist([id.A, id.B, X, id.C, id.D]);
    await user.click(row("Song B"));
    await user.keyboard("{Control>}");
    await user.click(row("Song D"));
    await user.keyboard("{/Control}");
    key("Delete");
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(savedIds(backend)).toEqual([id.A, X, id.C]);
    // With an Undo that puts the list back as it was.
    expect(app.toast).toMatch(/Removed 2 songs/);
    app.toastAction?.run();
    await settle();
    expect(savedIds(backend)).toEqual([id.A, id.B, X, id.C, id.D]);
  });

  test("the multi-song menu has Remove from this playlist, for all of them", async () => {
    const X = "gone-from-library";
    const { backend, user } = openPlaylist([id.A, id.B, X, id.C, id.D]);
    await user.click(row("Song A"));
    await user.keyboard("{Shift>}");
    await user.click(row("Song C"));
    await user.keyboard("{/Shift}");
    await user.pointer({ keys: "[MouseRight]", target: row("Song B") });
    await user.click(screen.getByRole("menuitem", { name: "Remove from this playlist" }));
    await settle();
    expect(savedIds(backend)).toEqual([X, id.D]);
  });

  test("Delete outside a playlist does nothing", async () => {
    const { backend, user } = openList();
    await user.click(row("Song B"));
    key("Delete");
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(0);
    expect(rows()).toHaveLength(6);
  });

  test("dragging one of several selected rows moves them together, in order", async () => {
    const X = "gone-from-library";
    const { backend, user } = openPlaylist([id.A, id.B, X, id.C, id.D, id.E]);
    await user.click(row("Song C"));
    await user.keyboard("{Control>}");
    await user.click(row("Song E"));
    await user.keyboard("{/Control}");
    const dataTransfer = { types: ["application/x-cadence-row"], setData() {}, effectAllowed: "" };
    await fireEvent.dragStart(row("Song E"), { dataTransfer });
    await fireEvent.dragOver(row("Song A"), { dataTransfer });
    await fireEvent.drop(row("Song A"), { dataTransfer });
    await settle();
    expect(savedIds(backend)).toEqual([id.C, id.E, id.A, id.B, X, id.D]);
  });

  test("a single row still drags by itself (not the selection it isn't part of)", async () => {
    const { backend, user } = openPlaylist([id.A, id.B, id.C, id.D]);
    await user.click(row("Song A"));
    await user.keyboard("{Control>}");
    await user.click(row("Song B"));
    await user.keyboard("{/Control}");
    const dataTransfer = { types: ["application/x-cadence-row"], setData() {}, effectAllowed: "" };
    await fireEvent.dragStart(row("Song D"), { dataTransfer });
    await fireEvent.dragOver(row("Song A"), { dataTransfer });
    await fireEvent.drop(row("Song A"), { dataTransfer });
    await settle();
    expect(savedIds(backend)).toEqual([id.D, id.A, id.B, id.C]);
  });
});

describe("Touch and Back", () => {
  const touchDown = (el: Element) => {
    const ev = new Event("pointerdown", { bubbles: true });
    Object.defineProperty(ev, "pointerType", { value: "touch" });
    el.dispatchEvent(ev);
  };

  test("long-press enters selection mode; taps then toggle rows; Back leaves the mode first", async () => {
    openList();
    app.history = [{ name: "albums" }];
    app.view = { name: "songs" };

    touchDown(row("Song B"));
    await fireEvent.contextMenu(row("Song B"));
    expect(app.menu).toBeNull(); // no menu: the bar has the actions
    expect(selected()).toEqual(["Song B"]);
    expect(screen.getByRole("toolbar", { name: "Selected songs" })).toBeInTheDocument();
    await fireEvent.click(row("Song B")); // the click that ends the long-press doesn't untick it

    touchDown(row("Song D"));
    await fireEvent.click(row("Song D"));
    expect(selected()).toEqual(["Song B", "Song D"]);
    touchDown(row("Song B"));
    await fireEvent.click(row("Song B"));
    expect(selected()).toEqual(["Song D"]);

    expect(app.handleBack()).toBe(true);
    flushSync();
    expect(selected()).toEqual([]);
    expect(app.view).toEqual({ name: "songs" }); // still here
    expect(screen.queryByRole("toolbar", { name: "Selected songs" })).not.toBeInTheDocument();

    // Out of selection mode a tap selects one song as before (and Back goes back).
    touchDown(row("Song C"));
    await fireEvent.click(row("Song C"));
    expect(selected()).toEqual(["Song C"]);
    expect(app.handleBack()).toBe(true);
    expect(app.history).toHaveLength(0);
  });
});

describe("Selection and list refreshes", () => {
  test("a library reload keeps the songs that are still there selected (at their new rows)", async () => {
    mockBackend();
    resetApp({ tracks, albums: [makeAlbum(tracks)] });
    const user = userEvent.setup();
    const { rerender } = render(TrackList, { tracks });
    await user.click(row("Song B"));
    await user.keyboard("{Shift>}");
    await user.click(row("Song D"));
    await user.keyboard("{/Shift}");
    expect(selected()).toEqual(["Song B", "Song C", "Song D"]);

    // Reloaded: C is gone, the rest come back as new objects in another order.
    const [A, B, , D, E, F] = tracks.map((t) => ({ ...t }));
    await rerender({ tracks: [D, A, E, B, F] });
    expect(selected()).toEqual(["Song D", "Song B"]);
  });

  test("another list (identity changed) starts with nothing selected", async () => {
    mockBackend();
    resetApp({ tracks, albums: [makeAlbum(tracks)] });
    const user = userEvent.setup();
    const { rerender } = render(TrackList, { tracks, identity: "one" });
    await user.click(row("Song B"));
    await rerender({ tracks: [...tracks], identity: "two" });
    expect(selected()).toEqual([]);
  });

  test("selecting in one list clears the other list's selection", async () => {
    mockBackend();
    resetApp({ tracks, albums: [makeAlbum(tracks)] });
    const user = userEvent.setup();
    render(TrackList, { tracks: tracks.slice(0, 3), label: "First" });
    render(TrackList, { tracks: tracks.slice(3), label: "Second" });
    const first = screen.getByRole("listbox", { name: "First" });
    const second = screen.getByRole("listbox", { name: "Second" });
    await user.click(within(first).getAllByRole("option")[0]);
    await user.click(within(second).getAllByRole("option")[0]);
    expect(within(first).queryAllByRole("option", { selected: true })).toHaveLength(0);
    expect(within(second).getAllByRole("option", { selected: true })).toHaveLength(1);
  });
});

describe("A 20,000-song list", () => {
  // "20,000 selected" (or 20.000, 20 000: the count is written the way the system writes numbers).
  const twentyThousand = /^20\D?000 selected$/;
  const big = () => Array.from({ length: 20_000 }, (_, i) => makeTrack({ title: `Track ${String(i).padStart(5, "0")}` }));

  test("Shift+click from the first row to the last, and Ctrl+A, are quick and build only the window", async () => {
    const songs = big();
    tracks = songs;
    openList(songs);
    const builtAtMost = 80;
    expect(rows().length).toBeLessThanOrEqual(builtAtMost);

    await fireEvent.click(rows()[0]);
    // Scroll to the end: jsdom has no layout, so the list says where it is, then the page scrolls.
    const el = list();
    el.getBoundingClientRect = () => ({ top: -(20_000 - 10) * 54, bottom: 0, left: 0, right: 0, width: 0, height: 0, x: 0, y: 0, toJSON() {} }) as DOMRect;
    document.documentElement.dispatchEvent(new Event("scroll"));
    flushSync();
    const lastRow = row("Track 19999");
    expect(rows().length).toBeLessThanOrEqual(builtAtMost);

    let t0 = performance.now();
    await fireEvent.click(lastRow, { shiftKey: true });
    flushSync();
    const shiftMs = performance.now() - t0;
    expect(screen.getByText(twentyThousand)).toBeInTheDocument();
    expect(rows().every((r) => r.getAttribute("aria-selected") === "true")).toBe(true);
    expect(rows().length).toBeLessThanOrEqual(builtAtMost);

    key("Escape");
    flushSync();
    expect(screen.queryByText(/selected$/)).not.toBeInTheDocument();

    t0 = performance.now();
    key("a", { ctrlKey: true });
    flushSync();
    const allMs = performance.now() - t0;
    expect(screen.getByText(twentyThousand)).toBeInTheDocument();
    expect(rows().length).toBeLessThanOrEqual(builtAtMost);

    // Generous bounds (a slow CI box); in practice each is a few milliseconds.
    expect(shiftMs).toBeLessThan(250);
    expect(allMs).toBeLessThan(250);
  });

  test("End moves to the last song and scrolls the page there (the window then builds it)", () => {
    const songs = big();
    tracks = songs;
    openList(songs);
    list().focus();
    key("Home");
    const before = document.documentElement.scrollTop;
    key("End");
    expect(document.documentElement.scrollTop).toBeGreaterThan(before);
    expect(list()).not.toHaveAttribute("aria-activedescendant"); // not built until the scroll lands
  });
});

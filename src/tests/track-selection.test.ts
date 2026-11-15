import { fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { flushSync } from "svelte";
import { beforeEach, describe, expect, test } from "vitest";
import TrackList from "#lib/components/TrackList.svelte";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeTrack, mockBackend, resetApp, settle } from "./helpers";
import { fx, key, list, openList, row, rows, selected, setupSongs } from "./selection-helpers";

// Selecting with the mouse and the keyboard, and what happens to a selection when the list changes.
// The right-click menu, playlists, drag and touch are in track-selection-actions.test.ts.
beforeEach(setupSongs);

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
    expect(selected()).toEqual(fx.tracks.map((t) => t.title));
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
    expect(selected()).toEqual(fx.tracks.map((t) => t.title));
  });

  test("Enter plays the active song (like double-click)", async () => {
    const { backend, user } = openList();
    await user.click(row("Song C"));
    key("Enter");
    await settle();
    expect(backend.last("play_tracks")).toEqual({ ids: fx.tracks.map((t) => t.id), start: 2, picked: true });
  });

  test("typing jumps to the next song whose title starts with it", () => {
    const list3 = [makeTrack({ title: "Alpha" }), makeTrack({ title: "Bravo" }), makeTrack({ title: "Banana" }), makeTrack({ title: "Charlie" })];
    fx.tracks = list3;
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
    fx.tracks = list3;
    openList(list3);
    list().focus();
    key("b");
    key("a");
    expect(selected()).toEqual(["Banana"]);
    await new Promise((r) => setTimeout(r, 900));
    key("c");
    expect(selected()).toEqual(["Charlie"]);
  });

  test("'/' and '?' right after a letter are left to the window (search, help), not typed into the jump", () => {
    const list3 = [makeTrack({ title: "Bravo" }), makeTrack({ title: "B/side" }), makeTrack({ title: "Charlie" })];
    fx.tracks = list3;
    openList(list3);
    list().focus();
    key("b");
    expect(selected()).toEqual(["Bravo"]);
    for (const k of ["/", "?"]) {
      const ev = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true });
      list().dispatchEvent(ev);
      expect(ev.defaultPrevented).toBe(false);
    }
    expect(selected()).toEqual(["Bravo"]);
  });

  test("the selection bar steps aside while Up Next or Now Playing is open, and comes back after", async () => {
    const { user } = openList();
    await user.click(row("Song A"));
    await user.keyboard("{Shift>}");
    await user.click(row("Song C"));
    await user.keyboard("{/Shift}");
    expect(screen.getByRole("toolbar", { name: "Selected songs" })).toBeInTheDocument();
    app.queueOpen = true;
    flushSync();
    expect(screen.queryByRole("toolbar", { name: "Selected songs" })).toBeNull();
    app.queueOpen = false;
    app.nowPlayingOpen = true;
    flushSync();
    expect(screen.queryByRole("toolbar", { name: "Selected songs" })).toBeNull();
    app.nowPlayingOpen = false;
    flushSync();
    expect(screen.getByRole("toolbar", { name: "Selected songs" })).toBeInTheDocument();
    expect(selected()).toHaveLength(3);
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

describe("Selection and list refreshes", () => {
  test("a library reload keeps the songs that are still there selected (at their new rows)", async () => {
    mockBackend();
    resetApp({ tracks: fx.tracks, albums: [makeAlbum(fx.tracks)] });
    const user = userEvent.setup();
    const { rerender } = render(TrackList, { tracks: fx.tracks });
    await user.click(row("Song B"));
    await user.keyboard("{Shift>}");
    await user.click(row("Song D"));
    await user.keyboard("{/Shift}");
    expect(selected()).toEqual(["Song B", "Song C", "Song D"]);

    // Reloaded: C is gone, the rest come back as new objects in another order.
    const [A, B, , D, E, F] = fx.tracks.map((t) => ({ ...t }));
    await rerender({ tracks: [D, A, E, B, F] });
    expect(selected()).toEqual(["Song D", "Song B"]);
  });

  test("another list (identity changed) starts with nothing selected", async () => {
    mockBackend();
    resetApp({ tracks: fx.tracks, albums: [makeAlbum(fx.tracks)] });
    const user = userEvent.setup();
    const { rerender } = render(TrackList, { tracks: fx.tracks, identity: "one" });
    await user.click(row("Song B"));
    await rerender({ tracks: [...fx.tracks], identity: "two" });
    expect(selected()).toEqual([]);
  });

  test("selecting in one list clears the other list's selection", async () => {
    mockBackend();
    resetApp({ tracks: fx.tracks, albums: [makeAlbum(fx.tracks)] });
    const user = userEvent.setup();
    render(TrackList, { tracks: fx.tracks.slice(0, 3), label: "First" });
    render(TrackList, { tracks: fx.tracks.slice(3), label: "Second" });
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
    fx.tracks = songs;
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
    fx.tracks = songs;
    openList(songs);
    list().focus();
    key("Home");
    const before = document.documentElement.scrollTop;
    key("End");
    expect(document.documentElement.scrollTop).toBeGreaterThan(before);
    expect(list()).not.toHaveAttribute("aria-activedescendant"); // not built until the scroll lands
  });
});

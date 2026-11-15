import { fireEvent, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { flushSync } from "svelte";
import { beforeEach, describe, expect, test } from "vitest";
import { app } from "#lib/store.svelte.js";
import { settle } from "./helpers";
import { fx, key, menuLabels, openList, openPlaylist, row, rows, savedIds, selected, setupSongs } from "./selection-helpers";

// What a selection is for: the right-click menu, playlist removal and drag, touch selection mode.
beforeEach(setupSongs);

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
    expect(backend.last("enqueue")).toEqual({ ids: [fx.id.B, fx.id.D], next: false });

    await user.pointer({ keys: "[MouseRight]", target: row("Song D") });
    await user.click(screen.getByRole("menuitem", { name: "Add to Playlist…" }));
    expect(app.picker?.map((t) => t.id)).toEqual([fx.id.B, fx.id.D]);
  });

  test("Edit Info… opens the tag editor for all of them, and is hidden when a CUE-sheet song is selected", async () => {
    fx.tracks[3] = { ...fx.tracks[3], cue: { start: 0, end: 100 } }; // Song D
    const { user } = openList();
    await user.click(row("Song A"));
    await user.keyboard("{Shift>}");
    await user.click(row("Song C"));
    await user.keyboard("{/Shift}");
    await user.pointer({ keys: "[MouseRight]", target: row("Song B") });
    await user.click(screen.getByRole("menuitem", { name: "Edit Info…" }));
    expect(app.tagEditing?.map((t) => t.id)).toEqual([fx.id.A, fx.id.B, fx.id.C]);

    await user.keyboard("{Shift>}");
    await user.click(row("Song D"));
    await user.keyboard("{/Shift}");
    await user.pointer({ keys: "[MouseRight]", target: row("Song D") });
    expect(menuLabels()).not.toContain("Edit Info…");
  });
});

describe("In a playlist", () => {
  test("a song listed twice: only the selected one of its two rows is removed", async () => {
    const { backend, user } = openPlaylist([fx.id.A, fx.id.B, fx.id.A, fx.id.C]);
    await user.click(row("Song A", 1)); // the second A
    key("Delete");
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(savedIds(backend)).toEqual([fx.id.A, fx.id.B, fx.id.C]);
  });

  test("Delete removes exactly the selected rows, by their places (a song missing from the library stays)", async () => {
    const X = "gone-from-library";
    const { backend, user } = openPlaylist([fx.id.A, fx.id.B, X, fx.id.C, fx.id.D]);
    await user.click(row("Song B"));
    await user.keyboard("{Control>}");
    await user.click(row("Song D"));
    await user.keyboard("{/Control}");
    key("Delete");
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(1);
    expect(savedIds(backend)).toEqual([fx.id.A, X, fx.id.C]);
    // With an Undo that puts the list back as it was.
    expect(app.toast).toMatch(/Removed 2 songs/);
    app.toastAction?.run();
    await settle();
    expect(savedIds(backend)).toEqual([fx.id.A, fx.id.B, X, fx.id.C, fx.id.D]);
  });

  test("Undo after another change puts the removed songs back into the list as it is now", async () => {
    const { backend, user } = openPlaylist([fx.id.A, fx.id.B, fx.id.C, fx.id.D]);
    await user.click(row("Song B"));
    key("Delete");
    await settle();
    expect(savedIds(backend)).toEqual([fx.id.A, fx.id.C, fx.id.D]);
    const undo = app.toastAction!;
    // Meanwhile a song is added to the end (Add to Playlist…).
    await app.addToPlaylist(app.playlists[0], [fx.tracks[5]]);
    expect(savedIds(backend)).toEqual([fx.id.A, fx.id.C, fx.id.D, fx.id.F]);
    undo.run();
    await settle();
    // B is back in its place, and F (added after) is kept.
    expect(savedIds(backend)).toEqual([fx.id.A, fx.id.B, fx.id.C, fx.id.D, fx.id.F]);
  });

  test("the multi-song menu has Remove from this playlist, for all of them", async () => {
    const X = "gone-from-library";
    const { backend, user } = openPlaylist([fx.id.A, fx.id.B, X, fx.id.C, fx.id.D]);
    await user.click(row("Song A"));
    await user.keyboard("{Shift>}");
    await user.click(row("Song C"));
    await user.keyboard("{/Shift}");
    await user.pointer({ keys: "[MouseRight]", target: row("Song B") });
    await user.click(screen.getByRole("menuitem", { name: "Remove from this playlist" }));
    await settle();
    expect(savedIds(backend)).toEqual([X, fx.id.D]);
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
    const { backend, user } = openPlaylist([fx.id.A, fx.id.B, X, fx.id.C, fx.id.D, fx.id.E]);
    await user.click(row("Song C"));
    await user.keyboard("{Control>}");
    await user.click(row("Song E"));
    await user.keyboard("{/Control}");
    const dataTransfer = { types: ["application/x-cadence-row"], setData() {}, effectAllowed: "" };
    await fireEvent.dragStart(row("Song E"), { dataTransfer });
    await fireEvent.dragOver(row("Song A"), { dataTransfer });
    await fireEvent.drop(row("Song A"), { dataTransfer });
    await settle();
    expect(savedIds(backend)).toEqual([fx.id.C, fx.id.E, fx.id.A, fx.id.B, X, fx.id.D]);
  });

  test("a single row still drags by itself (not the selection it isn't part of)", async () => {
    const { backend, user } = openPlaylist([fx.id.A, fx.id.B, fx.id.C, fx.id.D]);
    await user.click(row("Song A"));
    await user.keyboard("{Control>}");
    await user.click(row("Song B"));
    await user.keyboard("{/Control}");
    const dataTransfer = { types: ["application/x-cadence-row"], setData() {}, effectAllowed: "" };
    await fireEvent.dragStart(row("Song D"), { dataTransfer });
    await fireEvent.dragOver(row("Song A"), { dataTransfer });
    await fireEvent.drop(row("Song A"), { dataTransfer });
    await settle();
    expect(savedIds(backend)).toEqual([fx.id.D, fx.id.A, fx.id.B, fx.id.C]);
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

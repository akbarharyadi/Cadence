import { cleanup, fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import PlaylistPicker from "#lib/components/PlaylistPicker.svelte";
import QueuePanel from "#lib/components/QueuePanel.svelte";
import ArtistView from "#lib/views/ArtistView.svelte";
import type { Album, Playlist, Track } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makePlaylist, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

afterEach(() => cleanup());

describe("Artist page: order of albums", () => {
  // One artist, four albums: a year each, the last with none.
  const album = (title: string, year: number | null) => {
    const t = makeTrack({ title: `${title} song`, album: title, albumId: `id-${title}`, artist: "Mitski", albumArtist: "Mitski", year });
    return { track: t, album: makeAlbum([t], { year }) as Album };
  };
  const made = [album("Puberty 2", 2016), album("Be the Cowboy", 2018), album("Bury Me at Makeout Creek", 2014), album("Demo tape", null)];

  const shown = () =>
    screen
      .getAllByRole("group")
      .map((g) => g.getAttribute("aria-label") ?? "")
      .filter((l) => l.endsWith(" by Mitski"))
      .map((l) => l.replace(/ by Mitski$/, ""));
  function open() {
    mockBackend();
    resetApp({ tracks: made.map((m) => m.track), albums: made.map((m) => m.album) });
    return render(ArtistView, { name: "Mitski" });
  }

  test("newest first is the default (an album with no year goes last)", () => {
    open();
    expect(shown()).toEqual(["Be the Cowboy", "Puberty 2", "Bury Me at Makeout Creek", "Demo tape"]);
    expect(screen.getByRole("radio", { name: "Newest first" })).toHaveAttribute("aria-checked", "true");
  });

  test("oldest first, and A–Z; each choice is remembered for the next time", async () => {
    open();
    const user = userEvent.setup();

    await user.click(screen.getByRole("radio", { name: "Oldest first" }));
    expect(shown()).toEqual(["Bury Me at Makeout Creek", "Puberty 2", "Be the Cowboy", "Demo tape"]);
    expect(localStorage.getItem("cadence-artist-album-order")).toBe("oldest");

    await user.click(screen.getByRole("radio", { name: "A–Z" }));
    expect(shown()).toEqual(["Be the Cowboy", "Bury Me at Makeout Creek", "Demo tape", "Puberty 2"]);
    expect(localStorage.getItem("cadence-artist-album-order")).toBe("az");

    // Back to the page later: the saved order.
    cleanup();
    open();
    expect(shown()).toEqual(["Be the Cowboy", "Bury Me at Makeout Creek", "Demo tape", "Puberty 2"]);
    expect(screen.getByRole("radio", { name: "A–Z" })).toHaveAttribute("aria-checked", "true");
  });

  test("a bad saved value falls back to newest first; one album has no order to choose", () => {
    localStorage.setItem("cadence-artist-album-order", "sideways");
    open();
    expect(screen.getByRole("radio", { name: "Newest first" })).toHaveAttribute("aria-checked", "true");

    cleanup();
    const only = made[0];
    mockBackend();
    resetApp({ tracks: [only.track], albums: [only.album] });
    render(ArtistView, { name: "Mitski" });
    expect(screen.queryByRole("radiogroup", { name: "Order of albums" })).not.toBeInTheDocument();
  });
});

describe("Adding songs that are already in the playlist", () => {
  let tracks: Track[];
  let backend: ReturnType<typeof mockBackend>;
  const saved = () => (backend.last("save_playlist")!.playlist as Playlist).trackIds;

  /** Songs A to D; the playlist "Road trip" has A and B. `adding` are the songs being added. */
  function open(adding: number[], inPlaylist = [0, 1]) {
    backend = mockBackend();
    tracks = ["A", "B", "C", "D"].map((l) => makeTrack({ title: `Song ${l}` }));
    resetApp({ tracks, albums: [makeAlbum(tracks)], playlists: [makePlaylist(inPlaylist.map((i) => tracks[i].id))] });
    app.picker = adding.map((i) => tracks[i]);
    render(PlaylistPicker);
    return userEvent.setup();
  }
  const pick = (user: ReturnType<typeof userEvent.setup>) => user.click(screen.getByRole("button", { name: /Road trip/ }));
  const dialog = () => screen.getByRole("dialog", { name: "Already in this playlist" });

  test("nothing in common: no question, the songs are added", async () => {
    const user = open([2, 3]);
    await pick(user);
    await settle();
    expect(saved()).toEqual([tracks[0].id, tracks[1].id, tracks[2].id, tracks[3].id]);
    expect(app.picker).toBeNull();
  });

  test("some already there: 'N of M songs…', and Skip duplicates adds only the new ones", async () => {
    const user = open([1, 2, 3]);
    await pick(user);
    expect(dialog()).toHaveTextContent("1 of 3 songs are already in this playlist.");
    expect(backend.to("save_playlist")).toHaveLength(0);

    await user.click(within(dialog()).getByRole("button", { name: "Skip duplicates" }));
    await settle();
    expect(saved()).toEqual([tracks[0].id, tracks[1].id, tracks[2].id, tracks[3].id]);
    expect(app.picker).toBeNull();
  });

  test("Add anyway puts the song in a second time", async () => {
    const user = open([1, 2]);
    await pick(user);
    expect(dialog()).toHaveTextContent("1 of 2 songs");
    await user.click(within(dialog()).getByRole("button", { name: "Add anyway" }));
    await settle();
    expect(saved()).toEqual([tracks[0].id, tracks[1].id, tracks[1].id, tracks[2].id]);
    expect(app.picker).toBeNull();
  });

  test("Cancel saves nothing and leaves the picker open to choose another playlist", async () => {
    const user = open([0, 2]);
    await pick(user);
    await user.click(within(dialog()).getByRole("button", { name: "Cancel" }));
    await settle();
    expect(backend.to("save_playlist")).toHaveLength(0);
    expect(screen.queryByRole("dialog", { name: "Already in this playlist" })).not.toBeInTheDocument();
    expect(app.picker).not.toBeNull();
    expect(screen.getByRole("button", { name: /Road trip/ })).toBeInTheDocument();
  });

  test("Escape closes only the question", async () => {
    const user = open([0, 2]);
    await pick(user);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Already in this playlist" })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: /Add 2 songs/ })).toBeInTheDocument();
  });

  test("every song already there: no 'Skip duplicates' (it would add nothing), and Add anyway still works", async () => {
    const user = open([0, 1]);
    await pick(user);
    expect(dialog()).toHaveTextContent("All 2 songs are already in “Road trip”.");
    expect(within(dialog()).queryByRole("button", { name: "Skip duplicates" })).not.toBeInTheDocument();
    await user.click(within(dialog()).getByRole("button", { name: "Add anyway" }));
    await settle();
    expect(saved()).toEqual([tracks[0].id, tracks[1].id, tracks[0].id, tracks[1].id]);
  });

  test("one song already there: it names the song", async () => {
    const user = open([0]);
    await pick(user);
    expect(dialog()).toHaveTextContent("“Song A” is already in “Road trip”.");
  });
});

describe("Up Next: Jump to current", () => {
  // jsdom has no IntersectionObserver: a stand-in that the test drives.
  let observers: { callback: IntersectionObserverCallback; target?: Element; options?: IntersectionObserverInit }[];
  beforeEach(() => {
    observers = [];
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        entry: (typeof observers)[number];
        constructor(callback: IntersectionObserverCallback, options?: IntersectionObserverInit) {
          this.entry = { callback, options };
          observers.push(this.entry);
        }
        observe(el: Element) {
          this.entry.target = el;
        }
        disconnect() {}
        unobserve() {}
      },
    );
    mockBackend();
    const tracks = Array.from({ length: 80 }, () => makeTrack());
    resetApp({ tracks, albums: [makeAlbum(tracks)], status: makeStatus({ queue: tracks.map((t) => t.id), index: 5, duration: 200, playing: true }) });
  });
  afterEach(() => vi.unstubAllGlobals());

  const seen = (visible: boolean) => {
    const o = observers.at(-1)!;
    o.callback([{ isIntersecting: visible, target: o.target! } as IntersectionObserverEntry], {} as IntersectionObserver);
  };

  test("watches the playing song's row, inside the list", async () => {
    render(QueuePanel);
    await settle();
    const o = observers.at(-1)!;
    expect(o.target).toHaveClass("current");
    expect(o.options?.root).toHaveClass("scroll");
  });

  test("the button shows only while that row is out of view, and scrolls it back", async () => {
    const scrollTo = vi.spyOn(Element.prototype, "scrollTo").mockImplementation(() => {});
    render(QueuePanel);
    await settle();
    expect(screen.queryByRole("button", { name: "Jump to current" })).not.toBeInTheDocument();

    seen(false);
    await settle();
    const jump = screen.getByRole("button", { name: "Jump to current" });
    scrollTo.mockClear();
    await fireEvent.click(jump);
    expect(scrollTo).toHaveBeenCalledTimes(1);

    seen(true);
    await settle();
    expect(screen.queryByRole("button", { name: "Jump to current" })).not.toBeInTheDocument();
  });

  test("the panel opens at the playing song (once), and listens to no scrolling itself", async () => {
    const scrollTo = vi.spyOn(Element.prototype, "scrollTo").mockImplementation(() => {});
    const add = vi.spyOn(HTMLElement.prototype, "addEventListener");
    render(QueuePanel);
    await settle();
    expect(scrollTo).toHaveBeenCalledTimes(1);
    expect(add.mock.calls.filter(([type]) => type === "scroll")).toHaveLength(0);

    // The song changing doesn't drag the list along (no follow-the-song scrolling).
    scrollTo.mockClear();
    app.status = makeStatus({ ...app.status!, index: 6 });
    await settle();
    expect(scrollTo).not.toHaveBeenCalled();
  });
});

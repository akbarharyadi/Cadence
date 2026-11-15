// What the command palette lists and how it ranks it. No UI here: the component only shows it.
import { untrack } from "svelte";
import { api, type Track } from "./api";
import { PLACES } from "./places";
import { albumFields, artistFields, fold, ranked, trackFields, type Extra } from "./searchIndex";
import { extraMatch } from "./searchKeys";
import { keysOf } from "./shortcuts";
import { newSmart } from "./smart";
import { app, type Artist, type View } from "./store.svelte.js";
import { updates } from "./updates.svelte.js";
import { uiZoom } from "./uiZoom.svelte.js";
import { openHelp } from "./palette.svelte.js";
import { focusSearch } from "./focusSearch";

export type Kind = "page" | "action" | "album" | "artist" | "song";

export interface PaletteItem {
  id: string;
  kind: Kind;
  label: string;
  /** Dimmer text after the label (the artist of a song). */
  hint?: string;
  icon: string;
  keys?: string[];
  run: () => void;
  /** The label, folded: what typed text is matched against. */
  folded: string;
  /** Extra words that find it, folded. */
  words: string;
}

const make = (
  kind: Kind,
  id: string,
  label: string,
  icon: string,
  run: () => void,
  extra: { hint?: string; keys?: string[]; words?: string } = {},
): PaletteItem => ({
  id: `${kind}:${id}`,
  kind,
  label,
  icon,
  run,
  hint: extra.hint,
  keys: extra.keys,
  folded: fold(label),
  words: fold(extra.words ?? ""),
});

const android = () => typeof navigator !== "undefined" && navigator.userAgent.includes("Android");

/** The pages of the sidebar (and the phone's Library page), then each playlist. */
export function pageItems(): PaletteItem[] {
  const out = PLACES.map((p) => make("page", p.view.name, p.label, p.icon, () => app.go(p.view), { words: p.words }));
  for (const pl of untrack(() => app.playlists) ?? []) {
    out.push(make("page", `playlist:${pl.id}`, pl.name, pl.smart ? "smart" : "playlist", () => app.go({ name: "playlist", id: pl.id }), { hint: "Playlist", words: "playlist" }));
  }
  return out;
}

/** Things to do. Those that need a song playing are left out when none is (checked when the palette opens). */
export function actionItems(): PaletteItem[] {
  const playing = untrack(() => !!app.current);
  const phone = android();
  const a = (id: string, label: string, icon: string, run: () => void, shortcut?: string, words = "") =>
    make("action", id, label, icon, run, { keys: shortcut ? keysOf(shortcut) : undefined, words });
  return [
    a("toggle", "Play or pause", "play", () => app.togglePlay(), "toggle", "resume stop"),
    ...(playing
      ? [
          a("next", "Next song", "next", () => api.player("next"), "next", "skip"),
          a("prev", "Previous song", "prev", () => api.player("prev"), "prev", "back"),
        ]
      : []),
    a("shuffle", "Shuffle on or off", "shuffle", () => app.toggleShuffle(), "shuffle", "random"),
    a("repeat", "Repeat mode: off, all, this song", "repeat", () => app.cycleRepeat(), "repeat", "loop"),
    ...(playing
      ? [
          a("nowplaying", "Open Now Playing", "expand", () => (app.nowPlayingOpen = true), undefined, "player cover"),
          a(
            "lyrics",
            "Open lyrics",
            "lyrics",
            () => {
              app.lyricsOpen = true;
              app.nowPlayingOpen = true;
            },
            "lyrics",
            "words sing",
          ),
        ]
      : []),
    a("queue", "Open Up Next (queue)", "queue", () => (app.queueOpen = true), "queue", "queue playing next"),
    ...(phone ? [] : [a("mini", "Open mini player", "mini", () => api.toggleMini(), undefined, "small window")]),
    a("new-playlist", "New playlist", "playlist", () => app.newPlaylist(), undefined, "create"),
    a(
      "new-smart",
      "New smart playlist",
      "smart",
      () => (app.smartEditing = { id: "", name: "", trackIds: [], smart: newSmart(), created: 0, updated: 0 }),
      undefined,
      "create rules",
    ),
    a("rescan", "Rescan library", "refresh", () => api.rescan(), undefined, "look for changes import scan"),
    ...untrack(() => app.sleepItems()).map((m, i) =>
      a(`sleep:${i}`, /sleep/i.test(m.label) ? m.label : `Sleep timer: ${m.label}`, "moon", m.action, undefined, "sleep timer stop later night"),
    ),
    a("updates", "Check for updates", "download", () => void updates.check(), undefined, "new version upgrade"),
    a("search", "Search your music", "search", () => void focusSearch(), "search", "find"),
    a("help", "Keyboard shortcuts", "settings", () => openHelp(), "help", "keys hotkeys help"),
    ...(phone
      ? []
      : [
          a("zoom-in", "Larger interface", "plus", () => uiZoom.step(1), "zoom-in", "zoom size bigger text"),
          a("zoom-out", "Smaller interface", "minimize", () => uiZoom.step(-1), "zoom-out", "zoom size smaller text"),
          a("zoom-reset", "Normal interface size", "refresh", () => uiZoom.reset(), "zoom-reset", "zoom size reset"),
        ]),
  ];
}

/** What an empty box shows: the pages opened lately, and the actions that are used most. */
const USEFUL = ["toggle", "next", "prev", "shuffle", "queue", "new-playlist", "rescan"];
export function emptyItems(pages: PaletteItem[], actions: PaletteItem[]): { recent: PaletteItem[]; useful: PaletteItem[] } {
  const byView = new Map<string, PaletteItem>();
  for (const p of PLACES) {
    const item = pages.find((x) => x.id === `page:${p.view.name}`);
    if (item) byView.set(JSON.stringify(p.view), item);
  }
  for (const item of pages) if (item.id.startsWith("page:playlist:")) byView.set(JSON.stringify({ name: "playlist", id: item.id.slice(14) }), item);
  const recent: PaletteItem[] = [];
  const here = JSON.stringify(untrack(() => app.view));
  const history: View[] = untrack(() => app.history) ?? [];
  for (let i = history.length - 1; i >= 0 && recent.length < 4; i--) {
    const key = JSON.stringify(history[i]);
    const item = byView.get(key);
    if (item && key !== here && !recent.includes(item)) recent.push(item);
  }
  // Nothing opened yet (a fresh start): the places most people go to.
  if (!recent.length) {
    for (const name of ["songs", "albums", "favorites"]) {
      const item = byView.get(JSON.stringify({ name }));
      if (item) recent.push(item);
    }
  }
  return { recent, useful: USEFUL.map((id) => actions.find((x) => x.id === `action:${id}`)).filter((x): x is PaletteItem => !!x) };
}

/** Lower is better; null when a word of the text isn't there. Every word must match somewhere. */
export function score(item: PaletteItem, tokens: string[]): number | null {
  let total = 0;
  for (const t of tokens) {
    if (item.folded.startsWith(t)) total += 0;
    else if (item.folded.includes(` ${t}`)) total += 1;
    else if (item.folded.includes(t)) total += 2;
    else if (item.words.includes(t)) total += 3;
    else return null;
  }
  return total;
}

export function filterItems(items: PaletteItem[], tokens: string[], max: number): PaletteItem[] {
  const scored: { item: PaletteItem; s: number; i: number }[] = [];
  items.forEach((item, i) => {
    const s = score(item, tokens);
    if (s != null) scored.push({ item, s, i });
  });
  return scored
    .sort((a, b) => a.s - b.s || a.i - b.i)
    .slice(0, max)
    .map((x) => x.item);
}

/** The best `max` of a store list: the list is read here (a new one is searched again), its songs
 *  without subscribing to each (scanning 20,000 of them that way cost more than the search). */
const bestOf = <T>(list: T[] | undefined, fields: (x: T) => string[], q: string, max: number, extra?: Extra<T>): T[] =>
  list?.length ? untrack(() => ranked(list, fields, q, max, extra)) : [];

export const LIMITS = { pages: 6, actions: 6, albums: 4, artists: 4, songs: 6 };

/** Plays a song with the rest of its album after it; alone when its album isn't known. */
export function playSong(t: Track) {
  const album = app.albumById.get(t.albumId);
  const list = album ? app.albumTracks(album) : [];
  const at = list.findIndex((x) => x.id === t.id);
  if (at >= 0) app.playTracks(list, at, true);
  else app.playTracks([t], 0, true);
}

/** The library's best few matches, as items. `q` is the folded text. */
export function libraryItems(q: string): { albums: PaletteItem[]; artists: PaletteItem[]; songs: PaletteItem[] } {
  // Other names and readings ("yonezu" → 米津玄師) rank after every match on the names.
  const extra = extraMatch(untrack(() => app.searchKeys), q);
  const albums = bestOf(app.albums, albumFields, q, LIMITS.albums, extra?.album).map((al) =>
    make("album", al.id, al.title, "album", () => app.go({ name: "album", id: al.id }), { hint: al.artist }),
  );
  const artists = bestOf<Artist>(app.artists, artistFields, q, LIMITS.artists, extra?.artist).map((ar) =>
    make("artist", ar.name, ar.name, "artist", () => app.go({ name: "artist", artist: ar.name }), {
      hint: `${ar.albums.length} ${ar.albums.length === 1 ? "album" : "albums"}`,
    }),
  );
  const songs = bestOf<Track>(app.tracks, trackFields, q, LIMITS.songs, extra?.track).map((t) =>
    make("song", t.id, t.title, "song", () => playSong(t), { hint: `${t.artist} · ${t.album}` }),
  );
  return { albums, artists, songs };
}

export const GROUP_TITLE: Record<Kind, string> = { page: "Pages", action: "Actions", album: "Albums", artist: "Artists", song: "Songs" };
const KIND_ORDER: Kind[] = ["page", "action", "artist", "album", "song"];

export interface Group {
  kind: Kind | "recent" | "useful";
  title: string;
  items: PaletteItem[];
}

/** Everything the palette shows for the typed text (`raw`, any case), the best group first. */
export function search(raw: string, pages: PaletteItem[], actions: PaletteItem[]): Group[] {
  const text = fold(raw.trim()).replace(/\s+/g, " ");
  if (!text) {
    const { recent, useful } = emptyItems(pages, actions);
    const groups: Group[] = [
      { kind: "recent", title: "Recent", items: recent },
      { kind: "useful", title: "Actions", items: useful },
    ];
    return groups.filter((g) => g.items.length);
  }
  const tokens = text.split(" ");
  const lib = libraryItems(text);
  const byKind: Record<Kind, PaletteItem[]> = {
    page: filterItems(pages, tokens, LIMITS.pages),
    action: filterItems(actions, tokens, LIMITS.actions),
    album: lib.albums,
    artist: lib.artists,
    song: lib.songs,
  };
  const groups = KIND_ORDER.map((kind) => {
    const items = byKind[kind];
    // The group's rank is its best item's (library items match on other fields too: those rank last).
    const best = items.reduce((m, it) => Math.min(m, score(it, tokens) ?? 4), 9);
    return { kind, title: GROUP_TITLE[kind], items, best };
  }).filter((g) => g.items.length);
  return groups.sort((a, b) => a.best - b.best).map(({ kind, title, items }): Group => ({ kind, title, items }));
}

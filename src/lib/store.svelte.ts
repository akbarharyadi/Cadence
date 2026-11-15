import { listen } from "@tauri-apps/api/event";
import { listenHere } from "./windowEvents";
import { open } from "@tauri-apps/plugin-dialog";
import { flushSync, tick } from "svelte";
import { eqSync } from "./eq";
import { pageShowing, setWindowVisible } from "./showing.svelte.js";
import { closeTopModal } from "./components/Modal.svelte";
import { leaveSelection } from "./selection";
import { overlays } from "./palette.svelte.js";

/**
 * One request at a time: calls while one runs ask for exactly one more run after it (never
 * several), and all of them get the promise of the run that covers them. The last run's error,
 * if it failed, is passed on.
 */
export function coalesced(run: () => Promise<void>): () => Promise<void> {
  let current: Promise<void> | null = null;
  let again = false;
  return () => {
    if (current) {
      again = true;
      return current;
    }
    current = (async () => {
      try {
        let failed = false;
        let error: unknown;
        do {
          again = false;
          try {
            await run();
            failed = false;
          } catch (e) {
            failed = true;
            error = e;
          }
        } while (again);
        if (failed) throw error;
      } finally {
        current = null;
      }
    })();
    return current;
  };
}

/** Top-level places (tabs and sidebar): switching between them crossfades instead of sliding. */
const TOP_LEVEL = new Set(["home", "discover", "stats", "albums", "artists", "songs", "favorites", "health", "recent", "search", "sound", "settings", "library"]);
/** Pages with their own place in the sidebar (see `section`). */
const SECTIONS = new Set([...TOP_LEVEL, "attention"]);

let transitions = 0;

/** What the Android app offers the page (see MainActivity.Bridge). */
export interface AndroidBridge {
  update(json: string): void;
  installApk(path: string): string;
  hasMusicPermission(): boolean;
  requestMusicPermission(): void;
  getPlayOnBluetooth(): boolean;
  setPlayOnBluetooth(on: boolean): void;
  /** The colour behind the status and navigation bars (newer app versions). */
  setBackground?(color: string): void;
  /** SD cards and USB drives: JSON [{path, name}]. */
  storageVolumes?(): string;
  /** The song a file manager asked Cadence to play ("" if none), handed over once. */
  takeOpenedFile?(): string;
}

/** "album": colours from the song playing; "dark" / "black": the same for every song. */
export type Theme = "album" | "dark" | "black";
const DEFAULT_ACCENT = "#8b7cff";
const DEFAULT_TINT = "#1b1c33";
/** Desktop: the player as the window's top bar, or a bar along the bottom. */
export type PlayerPosition = "top" | "bottom";
function savedPlayerPosition(): PlayerPosition {
  return typeof localStorage !== "undefined" && localStorage.getItem("cadence-player-position") === "bottom" ? "bottom" : "top";
}
function savedTheme(): Theme {
  const t = typeof localStorage === "undefined" ? null : localStorage.getItem("cadence-theme");
  return t === "dark" || t === "black" ? t : "album";
}

/** The Android bridge, on phones (undefined on the desktop). */
/** What the system's own audio mixer is called here (shared output goes through it). */
export const systemMixer = (() => {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (ua.includes("Windows")) return "Windows mixer";
  if (ua.includes("Android")) return "Android mixer";
  if (ua.includes("Mac OS")) return "macOS mixer";
  return "system mixer";
})();

export function android(): AndroidBridge | undefined {
  return typeof window === "undefined" ? undefined : (window as unknown as { CadenceAndroid?: AndroidBridge }).CadenceAndroid;
}

/** How many pages Back (and Forward) remember. */
const MAX_HISTORY = 100;

/** A page left behind: where it was scrolled to, and the sidebar section it belonged to. */
interface HistoryEntry {
  view: View;
  scroll: number;
  section: string;
}

function sameView(a: View, b: View): boolean {
  try {
    return JSON.stringify(a) === JSON.stringify(b);
  } catch {
    return false;
  }
}

/** The page area's scroll offset. */
function contentScroll(): number {
  return (typeof document !== "undefined" && document.querySelector<HTMLElement>(".content")?.scrollTop) || 0;
}

/** Runs a view change inside a view transition (see app.css), when the system allows motion.
 *  `after` runs once the new page is in the document. */
function animate(kind: "forward" | "back" | "tab", change: () => void, after?: () => void) {
  const doc = document as Document & { startViewTransition?: (cb: () => void) => unknown };
  // Phones skip view transitions: snapshotting both pages stalls a frame for ~50 ms there. The
  // new page animates in by itself instead (.page-in in app.css), which costs nothing.
  if (!doc.startViewTransition || matchMedia("(prefers-reduced-motion: reduce)").matches || document.documentElement.classList.contains("phone")) {
    change();
    if (after) tick().then(after);
    return;
  }
  const root = document.documentElement;
  root.dataset.nav = kind;
  const run = ++transitions;
  const t = doc.startViewTransition(() => {
    flushSync(change);
    after?.();
  }) as { finished?: Promise<void> } | undefined;
  // The page's transition names (app.css) are only needed while it animates.
  t?.finished?.finally(() => {
    if (run === transitions) delete root.dataset.nav;
  });
}
import { announceInterrupted } from "./backupNotice";
import { api, artChanged, artistIndex, db, khz, type Album, type AiMix, type AiSettings, type AttentionState, type DspSettings, type DupGroup, type HealthProgress, type HealthReport, type HistoryStats, type JobProgress, type Mark, type Mix, type Playlist, type SonicStatus, type Snapshot, type Status, type Track, type VoiceBatch, type VoiceJob, type VoiceStatus, type ArtistGroup, type ArtistIdentity, type ArtistNamesStatus, type SearchKeys, type WaveformJob } from "./api";
import { deviceName } from "./devices";
import { throttled } from "./throttle";

/** One album (same name, same folder) shown as several: its songs have different album artists. */
export interface SplitAlbum {
  key: string;
  album: string;
  folder: string;
  /** Each album artist the songs are under, most songs first. */
  variants: { albumArtist: string; tracks: Track[] }[];
}

export type View =
  | { name: "home" }
  | { name: "albums" }
  | { name: "album"; id: string }
  | { name: "artists" }
  | { name: "artist"; artist: string }
  | { name: "songs" }
  | { name: "favorites" }
  | { name: "health" }
  | { name: "recent" }
  | { name: "attention" }
  | { name: "search"; query: string }
  | { name: "sound" }
  | { name: "playlist"; id: string }
  | { name: "similar"; id: string }
  | { name: "discover" }
  | { name: "stats" }
  | { name: "settings" }
  /** Phones: every place in the library on one page (the tab bar's Library). */
  | { name: "library" };

export interface MenuItem {
  label: string;
  icon?: string;
  action: () => void;
  /** A row of five stars instead of a label (the song's rating). */
  rating?: { value: number; set: (stars: number) => void };
}

export interface Artist {
  name: string;
  albums: Album[];
  trackCount: number;
  /** The artist's other spellings in the library (米津玄師 for Kenshi Yonezu): one artist, one page. */
  also?: string[];
}

class AppState {
  /** Replaced whole when the library changes, never changed in place: not deeply reactive, so
   *  20,000 songs aren't wrapped in proxies with a signal per field read (tens of MB). */
  tracks = $state.raw<Track[]>([]);
  albums = $state.raw<Album[]>([]);
  /** Replaced whole on every update (four times a second while playing), never changed in place. */
  status = $state.raw<Status | null>(null);
  /**
   * Whether this window's page can be seen (not hidden in the tray, minimized, or behind another
   * app on a phone): timers and animations run only while it is (see showing.svelte.ts).
   */
  get showing(): boolean {
    return pageShowing();
  }
  folders = $state<string[]>([]);
  scan = $state<{ done: number; total: number } | null>(null);
  /** Folders were just added (or looked at again) and the scan hasn't reported yet: Welcome says
   *  it's looking instead of "No music found". */
  adding = $state(false);
  private addingTimer: ReturnType<typeof setTimeout> | undefined;
  /** Marks the wait for a scan's first report (cleared by it, or after a minute at most). */
  startedAdding(on = true) {
    this.adding = on;
    clearTimeout(this.addingTimer);
    if (on) this.addingTimer = setTimeout(() => (this.adding = false), 60_000);
  }
  loaded = $state(false);

  view = $state<View>({ name: "home" });
  /** Counts page changes, and how the last one went, for the phone's page animation. */
  navSeq = $state(0);
  navKind = $state<"forward" | "back" | "tab">("tab");
  history = $state<View[]>([]);
  /** Pages Back went past, newest last: Forward (Alt+Right, mouse button 5) opens them again.
   *  Cleared when a new page is opened. */
  forwardHistory = $state<View[]>([]);
  /** The sidebar / tab bar place the page on screen belongs to: the top-level page (or playlist)
   *  it was opened from, so an album opened from Recently Added keeps Recently Added lit. */
  section = $state<string>("home");
  /** Scroll offsets and sections of the pages in `history` and `forwardHistory`, kept beside them. */
  private backMeta: Omit<HistoryEntry, "view">[] = [];
  private forwardMeta: Omit<HistoryEntry, "view">[] = [];
  nowPlayingOpen = $state(false);
  /** A song opened from another app: Now Playing opens when it starts. */
  opening = $state<string | null>(null);
  lyricsOpen = $state(false);
  queueOpen = $state(false);
  search = $state("");
  toast = $state<string | null>(null);
  /** A button in the toast (e.g. Undo). */
  toastAction = $state<{ label: string; run: () => void } | null>(null);
  dsp = $state<DspSettings | null>(null);
  analysis = $state<{ done: number; total: number } | null>(null);
  /** How much of the library has had sound analysis ("sounds like"). */
  sonic = $state<SonicStatus | null>(null);
  /** Following the singer: the models, and the song being listened to. */
  voice = $state<VoiceStatus | null>(null);
  /** The last song finished listening to (lyrics views reload when it's theirs), with any error. */
  voiceDone = $state<{ trackId: string; error: string; at: number } | null>(null);
  /** AI DJ settings, and whether an API key is saved. */
  ai = $state<{ settings: AiSettings; hasKey: boolean; ready?: boolean } | null>(null);
  aiReady = $derived(!!this.ai && this.ai.settings.provider === "cloud" && this.ai.hasKey);
  /** Today's AI mixes, and whether they're being made or failed. */
  aiMixes = $state<AiMix[]>([]);
  aiMixesState = $state<"idle" | "loading" | "error">("idle");
  /** Today's new-music picks (songs not in the library). */
  aiNew = $state<import("./api").NewSong[]>([]);
  /** Discover: the last search and what it found, and what the listener plays (for its choices). */
  discover = $state<import("./api").Recommendations | null>(null);
  /** Today's picks (This month), made once a day. */
  daily = $state<import("./api").Recommendations | null>(null);
  dailyState = $state<"idle" | "loading" | "error">("idle");
  dailyError = $state("");
  discoverState = $state<"idle" | "loading" | "error">("idle");
  discoverError = $state("");
  profile = $state<import("./api").ListeningProfile | null>(null);
  aiNewState = $state<"idle" | "loading" | "error">("idle");
  aiNewError = $state("");
  aiMixesError = $state("");
  /** Mixes of songs that sound alike. */
  mixes = $state<Mix[]>([]);
  menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);
  playlists = $state<Playlist[]>([]);
  /** Counts the times ListenBrainz refused Cadence's token (Settings reads the account again). */
  listenbrainzSignedOut = $state(0);
  /** Songs waiting for the "Add to playlist" picker. */
  picker = $state<Track[] | null>(null);
  /** Songs whose info (tags) is being edited. */
  tagEditing = $state<Track[] | null>(null);
  /** Pictures chosen for artists, by lower-case name. */
  artistImages = $state<Record<string, import("./api").ArtistImage>>({});
  /** Settings > Appearance. */
  theme = $state<Theme>(savedTheme());

  /** Settings > Appearance: where the player sits on the desktop. */
  playerPosition = $state<PlayerPosition>(savedPlayerPosition());
  setPlayerPosition(p: PlayerPosition) {
    this.playerPosition = p;
    localStorage.setItem("cadence-player-position", p);
  }

  /** Settings > Appearance: Now Playing's seek bar shows the song's waveform (on unless turned off). */
  waveformSeek = $state(typeof localStorage === "undefined" || localStorage.getItem("cadence-waveform-seek") !== "off");
  setWaveformSeek(on: boolean) {
    this.waveformSeek = on;
    localStorage.setItem("cadence-waveform-seek", on ? "on" : "off");
  }
  /** "Prepare waveforms for all songs", while it runs. */
  waveJob = $state<WaveformJob | null>(null);

  setTheme(theme: Theme) {
    this.theme = theme;
    localStorage.setItem("cadence-theme", theme);
  }

  /** Paints the window: album colours for the "album" theme, fixed ones otherwise. */
  paintTheme(album: Album | null) {
    const root = document.documentElement;
    const fixed = this.theme !== "album";
    root.dataset.theme = this.theme;
    root.style.setProperty("--accent", (!fixed && album?.accent) || DEFAULT_ACCENT);
    root.style.setProperty("--tint", fixed ? (this.theme === "black" ? "#000000" : DEFAULT_TINT) : (album?.tint ?? DEFAULT_TINT));
    android()?.setBackground?.(this.theme === "black" ? "#000000" : "#10111A");
  }

  /** Android: Cadence may read the music on the phone. */
  musicAccess = $state(true);
  /** The album whose cover is being changed. */
  coverEditing = $state<string | null>(null);
  /** The "Skipped songs" window is open. */
  skippedOpen = $state(false);
  /** The artist whose picture is being chosen. */
  artistEditing = $state<string | null>(null);
  /** The song open in Fix Song Info. */
  fixing = $state<Track | null>(null);
  /** The album open in Fix Album Info. */
  fixingAlbum = $state<Album | null>(null);
  /** Fix all (songs without info) while it runs. */
  fixAllJob = $state<JobProgress | null>(null);
  /** The check for artists under two names while it runs. */
  artistJob = $state<JobProgress | null>(null);
  /** Bumped when a library-fix job ends, so Needs Attention reloads what it shows. */
  fixJobsDone = $state(0);
  /** Songs Fix all found info for, waiting to be checked and saved. */
  fixAllFoundCount = $state(0);
  /** Artists under two names (from the artist check), for the sidebar's count. */
  artistPairsCount = $state(0);
  /** Bumped when Cadence's changes to files (the ones that can be undone) change. */
  changesVersion = $state(0);
  /** The open folder browser (Android): call with the chosen folder, or null if cancelled. */
  folderBrowser = $state<((path: string | null) => void) | null>(null);
  /** The smart playlist being edited (a copy), or null. */
  smartEditing = $state<Playlist | null>(null);

  trackById = $derived(new Map(this.tracks.map((t) => [t.id, t])));

  /** Needs Attention's lists, from the library as it is: albums split in two (one album, in one
   *  folder, under different album artists), songs without tags, and real albums without a cover,
   *  without a year or genre, or with songs missing (gaps in the track numbers). */
  attention = $derived.by(() => {
    const fold = (s: string) => s.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase().trim();
    const folder = (path: string) => path.replace(/[\\/][^\\/]*$/, "");
    const groups = new Map<string, Track[]>();
    const untagged: Track[] = [];
    for (const t of this.tracks) {
      // Songs without tags are grouped under their folder's name: they're listed on their own.
      if (this.needsInfo(t)) {
        untagged.push(t);
        continue;
      }
      if (!t.album.trim()) continue;
      const key = `${folder(t.path)}|${fold(t.album)}`;
      const g = groups.get(key);
      if (g) g.push(t);
      else groups.set(key, [t]);
    }
    const splits: SplitAlbum[] = [];
    for (const [key, tracks] of groups) {
      if (new Set(tracks.map((t) => t.albumId)).size < 2) continue;
      const by = new Map<string, Track[]>();
      for (const t of tracks) by.set(t.albumArtist, [...(by.get(t.albumArtist) ?? []), t]);
      const variants = [...by].map(([albumArtist, tracks]) => ({ albumArtist, tracks })).sort((a, b) => b.tracks.length - a.tracks.length);
      splits.push({ key, album: tracks[0].album, folder: folder(tracks[0].path), variants });
    }
    splits.sort((a, b) => a.album.localeCompare(b.album));
    // Real albums: not the "Unknown Artist" bucket of songs without tags.
    const real = this.albums.filter((a) => a.artist !== "Unknown Artist");
    const coverless = real.filter((a) => !a.art);
    const noYear = real.filter((a) => !a.year);
    const noGenre = real.filter((a) => !a.genre.trim());
    // Gaps in the track numbers of a disc: songs missing (3 or more there, at most half missing).
    const incomplete: { album: Album; missing: number[] }[] = [];
    for (const a of real) {
      const discs = new Map<number, Set<number>>();
      for (const id of a.trackIds) {
        const t = this.trackById.get(id);
        if (!t?.trackNo || t.cue) continue;
        const d = t.discNo ?? 1;
        (discs.get(d) ?? discs.set(d, new Set()).get(d)!).add(t.trackNo);
      }
      const missing: number[] = [];
      for (const nos of discs.values()) {
        const top = Math.max(...nos);
        if (nos.size < 3 || top > 99) continue;
        const gaps = Array.from({ length: top }, (_, i) => i + 1).filter((n) => !nos.has(n));
        if (gaps.length && gaps.length <= top / 2) missing.push(...gaps);
      }
      if (missing.length) incomplete.push({ album: a, missing });
    }
    return { splits, untagged, coverless, noYear, noGenre, incomplete };
  });

  /** Songs in the library more than once (Library Health's duplicates, and untagged downloads of
   *  songs the library has tagged). */
  duplicates = $state<DupGroup[]>([]);
  /** What the listener ignored and hid on Needs Attention. */
  attentionPrefs = $state<AttentionState>({ ignored: [], hidden: [] });
  ignoredSet = $derived(new Set(this.attentionPrefs.ignored));
  /** The search for covers of albums without one, while it runs. */
  coverJob = $state<JobProgress | null>(null);
  /** Covers found, waiting to be checked and saved. */
  coversFoundCount = $state(0);

  async loadDuplicates() {
    this.duplicates = await api.healthDuplicates();
  }
  async loadAttentionPrefs() {
    this.attentionPrefs = await api.attentionState();
  }
  async loadArtistPairsCount() {
    this.artistPairsCount = (await api.artistPairs()).pairs.length;
  }

  /** Every kind of problem on Needs Attention, worst first, each item with the key it's ignored
   *  under. `counted`: in the sidebar's number (things that are wrong, not just missing). */
  issues = $derived.by(() => {
    const ig = this.ignoredSet;
    const hidden = new Set(this.attentionPrefs.hidden);
    const keep = <T,>(kind: string, items: T[], key: (x: T) => string) => (hidden.has(kind) ? [] : items.filter((x) => !ig.has(`${kind}|${key(x)}`)));
    const a = this.attention;
    const damaged = keep("damaged", (this.health?.integrity ?? []).filter((r) => this.trackById.has(r.id)), (r) => r.id);
    const copies = keep("copies", this.duplicates, (g) => g.songs.find((s) => !s.keep)?.id ?? g.songs[0].id);
    // A song without tags that's a copy of one the library has is listed under copies, not here.
    const copyIds = new Set(this.duplicates.filter((g) => g.kind === "untagged").flatMap((g) => g.songs.filter((s) => !s.keep).map((s) => s.id)));
    const untagged = keep("untagged", a.untagged.filter((t) => !copyIds.has(t.id)), (t) => t.id);
    const splits = keep("splits", a.splits, (s) => s.key);
    const quality = keep("quality", (this.health?.quality ?? []).filter((r) => this.trackById.has(r.id)), (r) => r.id);
    const covers = keep("covers", a.coverless, (x) => x.id);
    const incomplete = keep("incomplete", a.incomplete, (x) => x.album.id);
    const year = keep("year", a.noYear, (x) => x.id);
    const genre = keep("genre", a.noGenre, (x) => x.id);
    return { damaged, copies, splits, untagged, quality, covers, incomplete, year, genre, hidden };
  });
  /** What the sidebar counts on Needs Attention: things that are wrong (damaged files, extra
   *  copies, split albums, artists under two names, songs without info). Missing covers, years
   *  and the like aren't counted: many libraries have some on purpose. */
  attentionCount = $derived(
    this.issues.damaged.length +
      this.issues.copies.length +
      this.issues.splits.length +
      this.issues.untagged.length +
      (this.issues.hidden.has("names") ? 0 : this.artistPairsCount),
  );
  albumById = $derived(new Map(this.albums.map((a) => [a.id, a])));
  /** Album-artist credit → the artists it's listed under (see `artistIndex`). */
  creditIndex = $derived(artistIndex(this.albums.map((a) => a.artist)));

  /** Artists the library writes in more than one way (from the app: one artist across scripts). */
  identity = $state.raw<ArtistIdentity | null>(null);
  /** Extra search words (other spellings, aliases, readings), from the app. */
  searchKeys = $state.raw<SearchKeys | null>(null);
  /** Learning artist names from MusicBrainz, while it runs. */
  namesJob = $state<ArtistNamesStatus | null>(null);
  /** Reading the listening history from ListenBrainz, while it runs. */
  lbImportJob = $state<import("./api").LbImportStatus | null>(null);
  /** Making the lyrics search's index (the first lyric search, then songs whose lyrics changed). */
  lyricIndexJob = $state<{ done: number; total: number } | null>(null);
  /** Bumped when the lyrics index is ready or updated (Search asks again). */
  lyricIndexAt = $state(0);
  /** Lower-case spelling → its artist's group in `identity`. */
  private groupOf = $derived.by(() => {
    const m = new Map<string, ArtistGroup>();
    for (const g of this.identity?.groups ?? []) for (const n of g.names ?? []) m.set(n.toLowerCase(), g);
    return m;
  });

  /** One entry per artist; albums credited to "A & B" appear under each real artist, and an
   *  artist written several ways (米津玄師, Kenshi Yonezu) is one entry under the spelling on most
   *  songs, with the others in `also`. */
  artists = $derived.by(() => {
    const map = new Map<string, Artist>();
    for (const a of this.albums) {
      for (const name of this.creditIndex.get(a.artist) ?? [a.artist]) {
        const key = name.toLowerCase();
        const entry = map.get(key) ?? { name, albums: [], trackCount: 0 };
        entry.albums.push(a);
        entry.trackCount += a.trackIds.length;
        map.set(key, entry);
      }
    }
    const merged = mergeSpellings([...map.values()], this.groupOf);
    return merged.sort((x, y) => x.name.localeCompare(y.name));
  });

  /** Any spelling (lower case) → its artist. */
  private artistIndexByName = $derived.by(() => {
    const m = new Map<string, Artist>();
    for (const a of this.artists) {
      m.set(a.name.toLowerCase(), a);
      for (const n of a.also ?? []) if (!m.has(n.toLowerCase())) m.set(n.toLowerCase(), a);
    }
    return m;
  });

  /** The artist written `name`, under any of its spellings. */
  artistByName(name: string): Artist | undefined {
    return this.artistIndexByName.get((name ?? "").toLowerCase());
  }

  /** The songs of the artist written `name`, under any of its spellings: the songs of their albums
   *  (as on the artist's page) and the songs credited to them on other albums, in library order. */
  songsOfArtist(name: string): Track[] {
    const a = this.artistByName(name);
    const names = new Set([name ?? "", ...(a ? [a.name, ...(a.also ?? [])] : [])].map((n) => n.toLowerCase()));
    const albums = new Set((a?.albums ?? []).map((x) => x.id));
    return this.tracks.filter((t) => albums.has(t.albumId) || names.has((t.artist ?? "").toLowerCase()));
  }

  /** The name of the artist page for `name` (its merged artist's shown spelling). */
  artistPageName(name: string): string {
    return this.artistByName(name)?.name ?? name;
  }

  /**
   * The artists written several ways, from the app. One request at a time (a burst of changes is
   * one more request after the running one), and an answer is only used when it is newer than the
   * one held: `version` only goes up, and an unchanged one leaves `artists` (and everything reading
   * it) alone. Search and the artist pages work without it (by the names as written).
   */
  loadIdentity(): Promise<void> {
    return this.identityRun();
  }
  private identityRun = coalesced(async () => {
    try {
      const id = await api.artistIdentity();
      if (!id || !Array.isArray(id.groups)) return;
      const have = this.identity;
      if (have && typeof id.version === "number" && typeof have.version === "number" && id.version <= have.version) return;
      this.identity = id;
    } catch {
      // Kept as it is.
    }
  });
  /** The app says the artists changed (the event carries the new version): asked only when it's new. */
  private identityChanged(version: unknown) {
    if (typeof version === "number" && this.identity && this.identity.version === version) return;
    void this.identityRun();
  }

  // ---------- the search keys (other spellings, readings), hundreds of KB for a big library ----------
  // Never at startup: the first time Search or the command palette opens. After that they are
  // marked out of date when the library or the artists change (asked again the next time Search or
  // the palette opens: a downloader writing files sends "library-changed" often), and asked at
  // once on "search-keys-changed" when Search or the palette is on screen.
  private keysWanted = false;
  private keysStale = true;
  private keysRun = coalesced(async () => {
    this.keysStale = false;
    try {
      const k = await api.searchKeys();
      // No answer: the keys held (if any) stay.
      if (k && typeof k === "object") this.searchKeys = k;
    } catch {
      this.keysStale = true;
    }
  });
  /** Search or the command palette opened: the search keys, if they aren't here or are out of date. */
  wantSearchKeys() {
    this.keysWanted = true;
    if (this.keysStale) void this.keysRun();
  }
  private searchKeysOutdated(now = false) {
    this.keysStale = true;
    if (now && this.keysWanted && (this.view.name === "search" || overlays.paletteOpen)) void this.keysRun();
  }

  /** The artist page for a credit: its lead artist. */
  artistOf(credit: string): string {
    // Under the spelling the artist's page is shown with (米津玄師 → Kenshi Yonezu when they're
    // one artist), so both spellings open the same page (and Back sees it as the same).
    return this.artistPageName(this.creditIndex.get(credit)?.[0] ?? credit);
  }

  /** The song and album last shown as playing: a rescan can drop a song that's still playing
   * (its file moved or was deleted as a duplicate), and the player keeps showing it until it ends. */
  private lastCurrent: Track | null = null;
  private lastAlbum: Album | null = null;

  current = $derived.by(() => {
    const s = this.status;
    if (!s || s.index == null) return null;
    const id = s.queue[s.index];
    const t = this.trackById.get(id);
    if (t) this.lastCurrent = t;
    return t ?? (this.lastCurrent?.id === id ? this.lastCurrent : null);
  });
  currentAlbum = $derived.by(() => {
    const t = this.current;
    if (!t) return null;
    const a = this.albumById.get(t.albumId);
    if (a) this.lastAlbum = a;
    return a ?? (this.lastAlbum?.id === t.albumId ? this.lastAlbum : null);
  });

  /** The album whose colours theme the app: the one playing, else the one on screen. */
  themeAlbum = $derived.by(() => this.currentAlbum ?? (this.view.name === "album" ? (this.albumById.get(this.view.id) ?? null) : null));

  /** The history again after plays ended, at most every 3 s: each answer (every played song's
   *  count) rebuilds "On repeat" and the recent albums, and with short songs a play ends every
   *  second. */
  private historyLater = throttled(() => api.history().then((h) => (this.listening = h)).catch(() => {}), 3000);
  /** Listening history stats (plays, skips, recent albums). */
  listening = $state.raw<HistoryStats | null>(null);
  /** Albums played this session, before the history has recorded them. */
  private sessionAlbums = $state<string[]>([]);
  /** Albums, most recently played first (this session, then the saved history). */
  recentlyPlayed = $derived([...new Set([...this.sessionAlbums, ...(this.listening?.recentAlbums ?? [])])].filter((id) => this.albumById.has(id)));
  /** Songs you've listened to most (skips don't count). */
  mostPlayed = $derived(
    Object.entries(this.listening?.plays ?? {})
      .filter(([id, n]) => n > 0 && this.trackById.has(id))
      .sort((a, b) => b[1] - a[1])
      .slice(0, 20)
      .map(([id]) => this.trackById.get(id)!),
  );

  /** Songs the user skips: left out when an album, playlist, mix or shuffle plays. */
  skipped = $state<string[]>([]);
  skippedSet = $derived(new Set(this.skipped));
  /** Also skip instrumental, karaoke and off-vocal versions. */
  skipInstrumentals = $state(false);

  /** Favourites and star ratings, by song id (songs with neither aren't listed). */
  /** Replaced whole on every change (`setMarks`): raw, so looking up 20,000 songs that have no
   *  mark doesn't leave a signal behind for each. */
  marks = $state.raw<Record<string, Mark>>({});
  ratingOf(t: Track): number {
    return this.marks[t.id]?.rating ?? 0;
  }
  isLoved(t: Track): boolean {
    return !!this.marks[t.id]?.loved;
  }
  /** Favourite songs, best rated first. */
  favorites = $derived(
    this.tracks
      .filter((t) => this.marks[t.id]?.loved)
      .sort((a, b) => this.ratingOf(b) - this.ratingOf(a) || a.title.localeCompare(b.title)),
  );

  private setMarks(tracks: Track[], change: Partial<Mark>) {
    const next = { ...this.marks };
    for (const t of tracks) {
      const m = { ...(next[t.id] ?? { rating: 0, loved: false }), ...change };
      if (m.rating === 0 && !m.loved) delete next[t.id];
      else next[t.id] = m;
    }
    this.marks = next;
  }

  async loadMarks() {
    this.marks = await api.marks();
  }

  /** Adds songs to the favourites, or takes them out. */
  setLoved(tracks: Track[], loved: boolean) {
    const ids = tracks.map((t) => t.id);
    this.setMarks(tracks, { loved });
    api.setLoved(ids, loved);
    this.showToast(loved ? `Added ${label(tracks, true)} to Favorites` : `Removed ${label(tracks, true)} from Favorites`, {
      label: "Undo",
      run: () => {
        this.setMarks(tracks, { loved: !loved });
        api.setLoved(ids, !loved);
        this.toast = this.toastAction = null;
      },
    });
  }

  toggleLoved(t: Track) {
    this.setLoved([t], !this.isLoved(t));
  }

  /** Rates songs 1–5 stars; the rating they already have clears it (0 clears too). */
  setRating(tracks: Track[], stars: number) {
    const rating = tracks.length === 1 && this.ratingOf(tracks[0]) === stars ? 0 : stars;
    this.setMarks(tracks, { rating });
    api.setRating(tracks.map((t) => t.id), rating);
  }

  /** An artist's picture: the one chosen for them, else their latest album's cover. */
  artistPicture(artist: Artist, size: "large" | "small"): string | null {
    // The picture chosen for the shown spelling, else one chosen for another spelling of the artist.
    const own = [artist.name, ...(artist.also ?? [])].map((n) => this.artistImages[n.toLowerCase()]).find(Boolean);
    if (own) return size === "large" ? own.large : own.small;
    const latest = [...artist.albums].filter((a) => a.art).sort((a, b) => (b.year ?? 0) - (a.year ?? 0))[0];
    return (size === "large" ? latest?.art : latest?.artSmall) ?? null;
  }

  async setArtistImage(name: string, source: { kind: "url"; url: string } | { kind: "file"; path: string }) {
    const img = await api.setArtistImage(name, source);
    this.artistImages = { ...this.artistImages, [name.toLowerCase()]: img };
    this.showToast(`New picture for ${name}`);
  }

  async resetArtistImage(name: string) {
    await api.resetArtistImage(name);
    const { [name.toLowerCase()]: _, ...rest } = this.artistImages;
    this.artistImages = rest;
  }

  async setAlbumCover(album: Album, source: { kind: "url"; url: string } | { kind: "file"; path: string }) {
    const r = await api.setAlbumCover(album.id, source);
    artChanged(r.art, r.artSmall);
    await this.loadLibrary();
    const notes = [r.later ? "the playing song is updated when it finishes" : "", r.failed ? `${r.failed} ${r.failed === 1 ? "file" : "files"} couldn't be changed` : ""].filter(Boolean);
    this.showToast(`New cover for “${album.title}”${notes.length ? `; ${notes.join(", ")}` : ""}`);
  }

  /** Songs with no real tags (e.g. downloads named "Artist - Title"). */
  needsInfo(t: Track): boolean {
    // (Songs of a single-file album get their info from the cue sheet, and can't be tagged one by one.)
    return t.artist === "Unknown Artist" && !t.cue;
  }

  /** Whether `t` is left out when a list plays (same rule as the engine's). */
  isSkipped(t: Track): boolean {
    return this.skippedSet.has(t.id) || (this.skipInstrumentals && isInstrumental(t.title));
  }

  async setSkipped(tracks: Track[], skip: boolean) {
    const ids = tracks.map((t) => t.id);
    this.skipped = skip ? [...new Set([...this.skipped, ...ids])] : this.skipped.filter((id) => !ids.includes(id));
    await api.setSkipped(ids, skip);
    this.showToast(skip ? `Skipping ${label(tracks, true)} from now on` : `No longer skipping ${label(tracks, true)}`, {
      label: "Undo",
      run: () => {
        this.skipped = skip ? this.skipped.filter((id) => !ids.includes(id)) : [...new Set([...this.skipped, ...ids])];
        api.setSkipped(ids, !skip);
        this.toast = this.toastAction = null;
      },
    });
  }

  // ---------- library health ----------

  health = $state<HealthReport | null>(null);
  /** The file check that's running (quality or integrity), while one is. */
  healthJob = $state<HealthProgress | null>(null);
  /** The day's snapshot of your data is being made (a second or so, at startup). */
  backupRunning = $state(false);

  async loadHealth() {
    this.health = await api.healthReport();
  }

  // ---------- sleep timer ----------

  /** When the sleep timer's minutes countdown ends (unix seconds), if one is running. */
  sleepEndsAt = $state<number | null>(null);
  /** A sleep timer is set: minutes, or "end of the song / album". */
  sleepActive = $derived(this.sleepEndsAt != null || !!this.status?.stopAfter);

  setSleep(kind: "off" | "minutes" | "songs" | "album", amount?: number) {
    api.setSleepTimer(kind, amount);
    this.sleepEndsAt = kind === "minutes" ? Math.floor(Date.now() / 1000) + (amount ?? 30) * 60 : null;
    this.showToast(
      kind === "off"
        ? "Sleep timer off"
        : kind === "minutes"
          ? `Sleep timer: pausing in ${amount} minutes`
          : kind === "album"
            ? "Sleep timer: pausing when this album ends"
            : (amount ?? 1) > 1
              ? `Sleep timer: pausing after ${amount} songs`
              : "Sleep timer: pausing when this song ends",
    );
  }

  /** The sleep timer's choices (for a menu). */
  sleepItems(): MenuItem[] {
    return [
      ...(this.sleepActive ? [{ label: "Turn Off Sleep Timer", icon: "close", action: () => this.setSleep("off") }] : []),
      ...[15, 30, 45, 60].map((m) => ({ label: `In ${m} minutes`, icon: "moon", action: () => this.setSleep("minutes", m) })),
      { label: "At the end of this song", icon: "moon", action: () => this.setSleep("songs", 1) },
      ...[3, 5, 10].map((n) => ({ label: `After ${n} songs`, icon: "moon", action: () => this.setSleep("songs", n) })),
      { label: "At the end of this album", icon: "moon", action: () => this.setSleep("album") },
    ];
  }

  // ---------- the queue ----------

  /** Empties Up Next (the song that's playing stays). */
  clearUpNext() {
    const s = this.status;
    if (!s || s.index == null) return;
    const ids = s.queue.slice(s.index + 1);
    if (!ids.length) return;
    api.clearUpNext();
    this.showToast(`Cleared ${ids.length} ${ids.length === 1 ? "song" : "songs"} from Up Next`, {
      label: "Undo",
      run: () => {
        api.enqueue(ids, false);
        this.toast = this.toastAction = null;
      },
    });
  }

  /** Takes the song at queue position `index` out; Undo puts it back in the same place. */
  removeFromQueue(index: number) {
    const id = this.status?.queue[index];
    if (!id) return;
    // The player keeps the song that's playing (it may have just started): nothing is removed.
    if (index === this.status?.index) return;
    const t = this.trackById.get(id);
    api.removeFromQueue(index);
    this.showToast(`Removed ${t ? `“${t.title}”` : "a song"} from Up Next`, {
      label: "Undo",
      run: () => {
        api.insertInQueue(index, [id]);
        this.toast = this.toastAction = null;
      },
    });
  }

  /** "Stop after this song" on the song at queue position `index`, or off again (one at a time). */
  toggleStopAfter(index: number) {
    const s = this.status;
    if (!s) return;
    const off = s.stopMark === index;
    api.stopAfterSong(off ? null : index);
    const t = this.trackById.get(s.queue[index] ?? "");
    this.showToast(off ? "Won’t stop after that song" : `Playback will stop after ${t ? `“${t.title}”` : "that song"}`);
  }

  /** Takes the songs Autoplay added out of Up Next. */
  clearAutoplay() {
    api.clearAutoplay();
    this.showToast("Cleared Autoplay’s songs");
  }

  /** The right-click menu of a song in Up Next (queue position `index`; the playing song too). */
  queueMenu(e: MouseEvent, index: number) {
    const s = this.status;
    const t = s ? this.trackById.get(s.queue[index] ?? "") : undefined;
    if (!s || !t) return;
    const current = index === s.index;
    const marked = s.stopMark === index;
    this.openMenu(e, [
      ...(current
        ? []
        : [
            { label: "Play Now", icon: "play", action: () => api.jump(index) },
            ...(s.index != null && index > s.index + 1 ? [{ label: "Play Next", icon: "playNext", action: () => api.moveInQueue(index, s.index! + 1) }] : []),
            { label: "Remove from Up Next", icon: "close", action: () => this.removeFromQueue(index) },
          ]),
      marked
        ? { label: "Don’t Stop After This Song", icon: "moon", action: () => this.toggleStopAfter(index) }
        : { label: "Stop After This Song", icon: "moon", action: () => this.toggleStopAfter(index) },
      { label: "Go to Album", icon: "album", action: () => this.go({ name: "album", id: t.albumId }) },
      { label: "Go to Artist", icon: "artist", action: () => this.go({ name: "artist", artist: this.artistOf(t.albumArtist) }) },
      { label: "Add to Playlist…", icon: "playlist", action: () => (this.picker = [t]) },
      this.isLoved(t)
        ? { label: "Remove from Favorites", icon: "heartFill", action: () => this.setLoved([t], false) }
        : { label: "Add to Favorites", icon: "heart", action: () => this.setLoved([t], true) },
    ]);
  }

  /** Saves the whole queue (played, playing and waiting songs) as a playlist. */
  async saveQueueAsPlaylist() {
    const tracks = (this.status?.queue ?? []).map((id) => this.trackById.get(id)).filter((t): t is Track => !!t);
    if (!tracks.length) return;
    const day = new Date().toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
    const p = await this.createPlaylist(`Queue, ${day}`, tracks);
    this.showToast(`Saved the queue as “${p.name}” (${tracks.length} ${tracks.length === 1 ? "song" : "songs"})`, {
      label: "Open",
      run: () => {
        this.go({ name: "playlist", id: p.id });
        this.toast = this.toastAction = null;
      },
    });
  }

  /** Shuffle the songs coming up (and lists played from now on), or put them back in order. */
  toggleShuffle() {
    const on = !this.status?.shuffle;
    api.setShuffle(on);
    this.showToast(on ? "Shuffle on" : "Shuffle off");
  }

  cycleRepeat() {
    const r = this.status?.repeat ?? "off";
    const next = r === "off" ? "all" : r === "all" ? "one" : "off";
    api.setRepeat(next);
    this.showToast(next === "all" ? "Repeat all" : next === "one" ? "Repeat this song" : "Repeat off");
  }

  setSkipInstrumentals(on: boolean) {
    this.skipInstrumentals = on;
    api.setSkipInstrumentals(on);
  }

  /**
   * Where playback is, as of the player's last update (four times a second while playing). Not
   * every frame: that made the window redraw up to 160 times a second (about 15% of a core) and
   * left garbage behind. Seek lines glide between updates with CSS.
   */
  position = $state(0);
  private posBase = { pos: 0, at: 0, playing: false };

  /** The position right now, between the player's updates. */
  exactPosition(): number {
    const b = this.posBase;
    const p = b.playing ? b.pos + (performance.now() - b.at) / 1000 : b.pos;
    const d = this.status?.duration ?? 0;
    return d > 0 ? Math.min(p, d) : p;
  }

  async init() {
    // All at once: each is a round trip to the app, and one after another they hold up the first screen.
    await Promise.all([
      // The queue (thousands of ids) only comes when it changed; until this window has one (from
      // `api.status()` below, or an update that brings it) updates without it are left alone.
      listenHere<Omit<Status, "queue"> & { queue?: string[] }>("player-status", (e) => {
        const queue = e.payload.queue ?? this.status?.queue;
        if (queue) this.applyStatus({ ...e.payload, queue });
      }),
      // Hidden in the tray or minimized, or shown again (after a fresh "player-status").
      listenHere<boolean>("window-visible", (e) => setWindowVisible(e.payload)),
      listen("library-changed", async () => {
        // The artists aren't asked for here: the app rebuilds them after a library change and says
        // so ("artist-identity-changed") only when they changed. The search keys are only marked
        // out of date (asked again when Search or the palette is on screen).
        this.searchKeysOutdated();
        if (!this.identity) void this.identityRun();
        await Promise.all([this.loadLibrary(), this.loadMarks()]);
        this.scan = null;
        this.startedAdding(false);
        await this.dropOwnedSuggestions();
        await Promise.all([this.loadDuplicates(), this.loadHealth(), this.loadArtistPairsCount()]);
      }),
      listen("attention-changed", () => this.loadAttentionPrefs()),
      listen<JobProgress>("cover-job-progress", (e) => {
        this.coverJob = e.payload.running ? e.payload : null;
        this.coversFoundCount = e.payload.found ?? 0;
        if (e.payload.running) return;
        this.fixJobsDone++;
        if (e.payload.total > 0) {
          const n = e.payload.found ?? 0;
          if (n) this.showToast(`Found covers for ${n} ${n === 1 ? "album" : "albums"}: check before saving`, { label: "Check", run: () => this.go({ name: "attention" }) }, 8000);
          else this.showToast("No covers found for those albums");
        }
      }),
      listen<{ done: number; total: number }>("library-progress", (e) => {
        this.scan = e.payload;
        if (this.adding) this.startedAdding(false);
      }),
      // A rescan that found nothing new: nothing to reload.
      listen("library-scanned", () => {
        this.scan = null;
        this.startedAdding(false);
      }),
      // A play ended: every second with very short songs or skipping, so gathered (see historyLater).
      listen("history-changed", () => this.historyLater()),
      listen("marks-changed", () => this.loadMarks()),
      listen<HealthProgress>("health-progress", (e) => (this.healthJob = e.payload.running ? e.payload : null)),
      listen<{ running: boolean }>("backup-progress", (e) => (this.backupRunning = !!e.payload?.running)),
      listen("health-done", () => this.loadHealth()),
      listen<JobProgress>("fix-all-progress", (e) => {
        this.fixAllJob = e.payload.running ? e.payload : null;
        this.fixAllFoundCount = e.payload.found ?? 0;
        if (e.payload.running) return;
        this.fixJobsDone++;
        // A finished run (saving or letting go of what it found also ends with an empty report).
        if (e.payload.total > 0) {
          const n = e.payload.found ?? 0;
          if (n) this.showToast(`Found the info of ${n} ${n === 1 ? "song" : "songs"}: check before saving`, { label: "Check", run: () => this.go({ name: "attention" }) }, 8000);
          else this.showToast("None of the songs without info were found in the catalogues");
        }
      }),
      listen("changes-changed", () => this.changesVersion++),
      // Files were replaced and the playlists follow them.
      listen("playlists-changed", () =>
        api.playlists().then((p) => (this.playlists = p)).catch(() => {}),
      ),
      // The token stopped working: Settings shows ListenBrainz as needing to be connected again.
      listen("listenbrainz-signed-out", () => {
        this.listenbrainzSignedOut++;
        this.showToast("ListenBrainz signed you out. Connect it again in Settings to keep sending your listens.", null, 8000);
      }),
      listen<JobProgress>("artist-check-progress", (e) => {
        this.artistJob = e.payload.running ? e.payload : null;
        if (e.payload.running) return;
        this.fixJobsDone++;
        this.loadArtistPairsCount();
      }),
      listen<number | null>("sleep-timer", (e) => (this.sleepEndsAt = e.payload)),
      // The artists changed (MusicBrainz learned names, or a "same" / "not the same" decision).
      listen<number>("artist-identity-changed", (e) => {
        this.identityChanged(e.payload);
        // The keys are worked out from the artists too.
        this.searchKeysOutdated();
      }),
      listen("search-keys-changed", () => this.searchKeysOutdated(true)),
      listen<ArtistNamesStatus>("artist-names-progress", (e) => (this.namesJob = e.payload?.running ? e.payload : null)),
      listen<import("./api").LbImportStatus>("listenbrainz-import-progress", (e) => (this.lbImportJob = e.payload?.phase === "scanning" ? e.payload : null)),
      listen<{ done: number; total: number; running: boolean }>("lyric-index-progress", (e) => (this.lyricIndexJob = e.payload?.running && e.payload.total > 0 ? e.payload : null)),
      listen("lyric-index-ready", () => (this.lyricIndexAt = Date.now())),
      listen<WaveformJob>("waveform-progress", (e) => (this.waveJob = e.payload?.running ? e.payload : null)),
      listen<{ done: number; total: number }>("analysis-progress", (e) => {
        this.analysis = e.payload.done >= e.payload.total ? null : e.payload;
      }),
      listen<VoiceJob>("voice-progress", (e) => {
        if (this.voice) this.voice = { ...this.voice, job: e.payload };
      }),
      listen<{ trackId: string; error: string }>("voice-done", (e) => {
        if (this.voice) this.voice = { ...this.voice, job: null };
        this.voiceDone = { ...e.payload, at: Date.now() };
      }),
      listen<VoiceBatch | null>("voice-batch", (e) => {
        if (this.voice) this.voice = { ...this.voice, batch: e.payload };
      }),
      listen<[number, number]>("voice-install", async (e) => {
        const [done, total] = e.payload;
        if (total && this.voice) this.voice = { ...this.voice, installing: [done, total] };
        else this.voice = await api.voiceStatus();
      }),
      listen<{ done: number; total: number }>("sonic-progress", (e) => {
        const { done, total } = e.payload;
        if (done >= total) this.loadSonic();
        else if (this.sonic) this.sonic = { ...this.sonic, analyzed: this.sonic.total - total + done, running: true };
      }),
    ]);
    this.loadSonic();
    api.voiceStatus().then((v) => (this.voice = v)).catch(() => {});
    api.aiSettings().then((a) => {
      this.ai = a;
      if (this.aiReady) {
        this.loadAiMixes(false);
        this.loadAiNew(false);
      }
    }).catch(() => {});
    api.artistImages().then((m) => (this.artistImages = m)).catch(() => {});
    // The mini player is another window: follow a theme changed in the main one.
    window.addEventListener("storage", (e) => e.key === "cadence-theme" && (this.theme = savedTheme()));
    // Whether a pointer is held (a sound slider may be mid-drag): see `applyDeviceDsp`.
    this.watchPointer();
    this.loadMarks();
    api.sleepTimer().then((t) => (this.sleepEndsAt = t)).catch(() => {});
    // The day's snapshot may have started before this window was listening.
    api.backupInfo().then((b) => {
      if (b?.running) this.backupRunning = true;
      // A restore cut short was dealt with at this start: said once (the Backup section keeps it).
      announceInterrupted(b, (m) => this.showToast(m, null, 12000));
    }).catch(() => {});
    api.skips().then((s) => {
      this.skipped = s.ids;
      this.skipInstrumentals = s.instrumentals;
    }).catch(() => {});
    const [lib, folders, status, dsp, history, playlists] = await Promise.all([
      api.library(),
      api.folders(),
      api.status(),
      api.dsp(),
      api.history(),
      api.playlists(),
    ]);
    this.playlists = playlists;
    this.listening = history;
    this.setLibrary(lib);
    void this.loadIdentity();
    this.folders = folders;
    // An update that came in meanwhile is newer.
    if (!this.status) this.applyStatus(status);
    this.dsp = dsp;
    this.loaded = true;
    // Needs Attention's count, after the first screen is up.
    setTimeout(() => {
      this.loadAttentionPrefs();
      this.loadDuplicates();
      this.loadHealth();
      this.loadArtistPairsCount();
    }, 1500);
  }

  /**
   * For the mini player window: only what it shows (the player's status, the song and album
   * playing, the theme). The library comes once (the playing song is looked up in it) and again
   * when it changes; none of the main window's lists, history, sonic data or AI mixes are loaded.
   */
  async initMini() {
    await Promise.all([
      listenHere<Omit<Status, "queue"> & { queue?: string[] }>("player-status", (e) => {
        const queue = e.payload.queue ?? this.status?.queue;
        if (queue) this.applyStatus({ ...e.payload, queue });
        this.miniFollow();
      }),
      listenHere<boolean>("window-visible", (e) => setWindowVisible(e.payload)),
      listen("library-changed", () => this.miniFollow(true)),
    ]);
    window.addEventListener("storage", (e) => e.key === "cadence-theme" && (this.theme = savedTheme()));
    const status = await api.status();
    if (!this.status) this.applyStatus(status);
    await this.miniFollow();
    this.loaded = true;
  }

  /** The mini player's songs: the one playing and the next (so a song change shows at once), not
   *  the whole library (tens of MB for 20,000 songs). Asked again when they change. */
  private miniFollow(changed = false): Promise<void> {
    const s = this.status;
    const ids = s?.index != null ? s.queue.slice(s.index, s.index + 2) : [];
    const key = ids.join("\n");
    if (!changed && key === this.miniKey) return Promise.resolve();
    this.miniKey = key;
    return this.miniRun().catch(() => {});
  }
  private miniKey: string | null = null;
  private miniRun = coalesced(async () => {
    const lib = await api.libraryPart(this.miniKey ? this.miniKey.split("\n") : []);
    if (lib && Array.isArray(lib.tracks) && Array.isArray(lib.albums)) this.setLibrary(lib);
  });

  private applyStatus(s: Status) {
    this.status = s;
    const albumId = s.index != null ? this.trackById.get(s.queue[s.index])?.albumId : undefined;
    if (albumId && this.sessionAlbums[0] !== albumId) this.sessionAlbums = [albumId, ...this.sessionAlbums.filter((a) => a !== albumId)].slice(0, 20);
    this.posBase = { pos: s.position, at: performance.now(), playing: s.playing };
    this.position = this.exactPosition();
    eqSync();
    if (s.error && s.error !== this.lastError) this.showToast(s.error);
    this.lastError = s.error;
    this.followDevice(s);
    if (this.startSeek) this.seekOnStart(s);
  }
  private lastError: string | null = null;

  /**
   * The output device's own sound settings came into use (another device, or Cadence started on
   * one): the Sound page shows them. The chosen device went missing: said once.
   */
  private followDevice(s: Status) {
    const profile = s.device ? `${s.device.id}\n${s.device.profile}` : null;
    if (profile !== this.lastProfile) {
      const first = this.lastProfile === undefined;
      this.lastProfile = profile;
      if (!first && this.dsp) {
        // What the page holds is the old device's settings: nothing of it may reach the engine now
        // (it would be saved in the new device's profile). An edit waiting to be sent is dropped,
        // and edits wait until the new device's settings are here.
        clearTimeout(this.dspTimer);
        this.dspTimer = undefined;
        this.dspStale = true;
        this.reloadDsp();
      }
    }
    const missing = s.deviceMissing?.name ?? null;
    if (missing && missing !== this.lastMissing) {
      const fallback = s.deviceMissing?.fallback;
      this.showToast(
        `${deviceName(missing)} isn’t connected, so playback is paused`,
        fallback ? { label: `Play on ${deviceName(fallback)}`, run: () => api.outputUseDefault().catch(() => {}) } : null,
        10000,
      );
    }
    this.lastMissing = missing;
  }
  private lastProfile: string | null | undefined = undefined;
  private lastMissing: string | null = null;

  /** The library again. While the downloader writes files "library-changed" comes often: one
   *  request at a time, and a burst during one is one more after it. */
  loadLibrary(): Promise<void> {
    return this.libraryRun();
  }
  private libraryRun = coalesced(async () => {
    const lib = await api.library();
    if (lib && Array.isArray(lib.tracks) && Array.isArray(lib.albums)) this.setLibrary(lib);
  });

  async loadSonic() {
    const [status, mixes] = await Promise.all([api.sonicStatus(), api.soundMixes()]);
    this.sonic = status;
    this.mixes = mixes;
  }

  async loadAiMixes(refresh: boolean) {
    if (!this.aiReady || this.aiMixesState === "loading") return;
    this.aiMixesState = "loading";
    try {
      this.aiMixes = await api.aiMixes(refresh);
      this.aiMixesState = "idle";
    } catch (e) {
      this.aiMixesError = String(e);
      this.aiMixesState = "error";
    }
  }

  /** Discover's page opened: the listener's top artists and genres, and the last search. */
  async loadDiscover() {
    this.profile = await api.listeningProfile();
    if (!this.discover) {
      const last = await api.aiRecommended();
      if (last.picks.length) this.discover = last;
    }
  }

  /** Today's picks: from the day's cache, or found now (once a day; `refresh` for new ones). */
  async loadDaily(refresh: boolean) {
    if (!this.aiReady || this.dailyState === "loading" || this.aiNewState === "loading") return;
    this.dailyState = "loading";
    this.dailyError = "";
    // Home's "New music for you" shows the same list (see loadAiNew).
    this.aiNewState = "loading";
    try {
      this.daily = await api.aiDailyPicks(refresh);
      this.aiNew = this.daily.picks;
      this.dailyState = "idle";
      this.aiNewState = "idle";
    } catch (e) {
      this.dailyError = String(e);
      this.dailyState = "error";
      this.aiNewError = String(e);
      this.aiNewState = "error";
    }
  }

  /** The library changed: Discover's and Home's suggestions without the songs it has now. */
  async dropOwnedSuggestions() {
    const [daily, found, aiNew] = [this.daily, this.discover, this.aiNew];
    const still = (picks: import("./api").NewSong[]) => (picks.length ? api.aiStillNew(picks) : Promise.resolve(picks));
    try {
      const [d, f, n] = await Promise.all([still(daily?.picks ?? []), still(found?.picks ?? []), still(aiNew)]);
      // Unless new ones came in meanwhile.
      if (daily && this.daily === daily) this.daily = { ...daily, picks: d };
      if (found && this.discover === found) this.discover = { ...found, picks: f };
      if (this.aiNew === aiNew) this.aiNew = n;
    } catch {
      // Kept as they are.
    }
  }

  /** Discover: new songs for this month's listening, an artist, a genre or a request in words. */
  async recommend(basis: import("./api").DiscoverBasis, value: string) {
    if (!this.aiReady || this.discoverState === "loading") return;
    this.discoverState = "loading";
    this.discoverError = "";
    try {
      this.discover = await api.aiRecommend(basis, value);
      this.discoverState = "idle";
    } catch (e) {
      this.discoverError = String(e);
      this.discoverState = "error";
    }
  }

  async loadAiNew(refresh: boolean) {
    if (!this.aiReady || this.aiNewState === "loading" || this.dailyState === "loading") return;
    this.aiNewState = "loading";
    // Home's "New music for you" and Discover's "Today's picks" are the same list (the engine's
    // ai_discover is ai_daily_picks without the details): both are set from one answer, so new
    // ones found here show on Discover too.
    this.dailyState = "loading";
    this.dailyError = "";
    try {
      const daily = await api.aiDailyPicks(refresh);
      this.daily = daily;
      this.aiNew = daily.picks;
      this.aiNewState = "idle";
      this.dailyState = "idle";
    } catch (e) {
      this.aiNewError = String(e);
      this.aiNewState = "error";
      this.dailyError = String(e);
      this.dailyState = "error";
    }
  }

  async saveAiSettings(settings: AiSettings) {
    await api.setAiSettings($state.snapshot(settings) as AiSettings);
    if (this.ai) this.ai = { ...this.ai, settings };
  }

  /** Plays songs that sound like `t`, starting with it. */
  async soundRadio(t: Track) {
    const ids = await api.soundRadio(t.id);
    if (ids.length <= 1) {
      this.showToast(this.sonic?.running ? "Cadence is still listening to your songs. Try again in a minute." : `Not enough analysed songs to make a radio from “${t.title}”.`);
      return;
    }
    await api.play(ids, 0);
    this.showToast(`Playing songs that sound like “${t.title}”`);
  }

  mixTracks(mix: Mix): Track[] {
    return mix.trackIds.map((id) => this.trackById.get(id)).filter((t): t is Track => !!t);
  }

  private setLibrary(lib: Snapshot) {
    this.tracks = lib.tracks;
    this.albums = lib.albums;
  }

  /** The section a page starts: top-level pages and playlists are their own, others keep the
   *  one they were opened from. */
  private sectionOf(view: View): string {
    if (SECTIONS.has(view.name)) return view.name;
    if (view.name === "playlist") return `playlist:${view.id}`;
    return this.section;
  }

  /** Shows a page. Leaving the search page empties the sidebar's search box; the box is never
   *  written here otherwise, so text typed while a page change was waiting for a frame survives
   *  (back() and forward() set it from the history entry). */
  private setView(view: View) {
    const leavingSearch = this.view.name === "search" && view.name !== "search";
    this.view = view;
    if (leavingSearch && this.search !== "") this.search = "";
  }

  /** A page change started by typing in the search box that hasn't run yet (view transitions
   *  wait a frame): more keystrokes in that time only update it, never push history again. */
  private pendingSearch = false;

  private remember(stack: View[], meta: Omit<HistoryEntry, "view">[], entry: HistoryEntry) {
    stack.push(entry.view);
    meta.push({ scroll: entry.scroll, section: entry.section });
    if (stack.length > MAX_HISTORY) {
      stack.splice(0, stack.length - MAX_HISTORY);
      meta.splice(0, meta.length - MAX_HISTORY);
    }
  }

  /** Puts a page back where it was scrolled to once it has rendered (its lists can take a moment
   *  to reach that height; gives up after half a second, or when another page is opened). */
  private restoreScroll(top: number) {
    if (top <= 0) {
      // The page was at its top: don't keep the previous page's offset.
      const el = document.querySelector<HTMLElement>(".content");
      if (el) el.scrollTop = 0;
      return;
    }
    const seq = this.navSeq;
    let tries = 0;
    const attempt = () => {
      if (seq !== this.navSeq) return;
      const el = document.querySelector<HTMLElement>(".content");
      if (!el) return;
      el.scrollTop = top;
      if (Math.abs(el.scrollTop - top) > 2 && ++tries < 10) setTimeout(attempt, 50);
    };
    attempt();
  }

  go(view: View) {
    // Background listening (lyrics follow the singer) gives way while the page changes.
    api.uiBusy().catch(() => {});
    // Typing in the search box updates the same page: no animation for that.
    if (view.name === "search" && this.view.name === "search") {
      this.setView(view);
      return;
    }
    // The page that's already open (Albums pressed again): back to its top, nothing to go back to.
    if (sameView(view, this.view)) {
      this.nowPlayingOpen = false;
      document.querySelector(".content")?.scrollTo({ top: 0, behavior: "smooth" });
      return;
    }
    const fromTyping = view.name === "search";
    if (fromTyping) {
      if (this.pendingSearch) return; // the waiting page change reads the box when it runs
      this.pendingSearch = true;
    }
    const kind = TOP_LEVEL.has(view.name) ? "tab" : "forward";
    const here: HistoryEntry = { view: this.view, scroll: contentScroll(), section: this.section };
    const change = () => {
      if (fromTyping) {
        this.pendingSearch = false;
        // Letters typed while waiting are in the box, not in `view`.
        if (this.search.trim()) view = { name: "search", query: this.search };
      }
      this.remember(this.history, this.backMeta, here);
      this.forwardHistory = [];
      this.forwardMeta = [];
      this.section = this.sectionOf(view);
      this.setView(view);
      this.navKind = kind;
      this.navSeq++;
      this.nowPlayingOpen = false;
      document.querySelector(".content")?.scrollTo({ top: 0 });
    };
    try {
      animate(kind, () => {
        try {
          change();
        } finally {
          // Whatever happened, the search box can open Search again.
          if (fromTyping) this.pendingSearch = false;
        }
      });
    } catch (e) {
      // The transition couldn't start (or the change threw without one): never left waiting.
      if (fromTyping) this.pendingSearch = false;
      throw e;
    }
  }

  back() {
    if (!this.history.length) return;
    const prev = this.history.pop()!;
    const meta = this.backMeta.pop() ?? { scroll: 0, section: this.sectionOf(prev) };
    const here: HistoryEntry = { view: this.view, scroll: contentScroll(), section: this.section };
    animate(
      "back",
      () => {
        this.remember(this.forwardHistory, this.forwardMeta, here);
        this.section = meta.section;
        this.setView(prev);
        if (prev.name === "search") this.search = prev.query;
        this.navKind = "back";
        this.navSeq++;
        this.nowPlayingOpen = false;
      },
      () => this.restoreScroll(meta.scroll),
    );
  }

  /** Opens again the page Back left (Alt+Right, the mouse's forward button). */
  forward() {
    if (!this.forwardHistory.length) return;
    const next = this.forwardHistory.pop()!;
    const meta = this.forwardMeta.pop() ?? { scroll: 0, section: this.sectionOf(next) };
    const here: HistoryEntry = { view: this.view, scroll: contentScroll(), section: this.section };
    const kind = TOP_LEVEL.has(next.name) ? "tab" : "forward";
    animate(
      kind,
      () => {
        this.remember(this.history, this.backMeta, here);
        this.section = meta.section;
        this.setView(next);
        if (next.name === "search") this.search = next.query;
        this.navKind = kind;
        this.navSeq++;
        this.nowPlayingOpen = false;
      },
      () => this.restoreScroll(meta.scroll),
    );
  }

  /** Puts the files of one of Cadence's changes back as they were. */
  async undoChange(id: string) {
    try {
      const n = await api.undoChange(id);
      this.showToast(`Undone: ${n === 1 ? "the song is" : `${n} songs are`} back as before`);
    } catch (e) {
      this.showToast(String(e));
    }
  }

  /** The Undo button for a toast about a change (none when there's nothing to undo). */
  undoAction(change: string | null | undefined): { label: string; run: () => void } | null {
    return change ? { label: "Undo", run: () => this.undoChange(change) } : null;
  }

  showToast(message: string, action: { label: string; run: () => void } | null = null, ms?: number) {
    this.toast = message;
    this.toastAction = action;
    clearTimeout(this.toastTimer);
    // Long messages stay long enough to read (about 60 ms a character, at most 15 s).
    const read = Math.min(15_000, message.length * 60);
    this.toastTimer = setTimeout(() => (this.toast = this.toastAction = null), ms ?? Math.max(action ? 6000 : 4000, read));
  }
  private toastTimer: ReturnType<typeof setTimeout> | undefined;

  albumTracks(album: Album): Track[] {
    return album.trackIds.map((id) => this.trackById.get(id)).filter((t): t is Track => !!t);
  }

  /** Plays a list; skipped songs are left out, except the start song when the user `picked` it. */
  playTracks(tracks: Track[], start = 0, picked = false) {
    if (tracks.length === 0) return;
    api.play(tracks.map((t) => t.id), start, picked);
  }

  /** Plays a list shuffled by the player (the balanced shuffle, or fully random, as set in Sound). */
  shuffleTracks(tracks: Track[]) {
    if (tracks.length === 0) return;
    api.playShuffled(tracks.map((t) => t.id));
  }

  /** Album shuffle: a random album, whole and in order, then another, each once. */
  async shuffleAlbums(albums: Album[]) {
    if (!albums.length) return;
    const ok = await api.shuffleAlbums(albums.map((a) => a.id)).catch(() => false);
    this.showToast(ok ? `Shuffling ${albums.length} ${albums.length === 1 ? "album" : "albums"}, one whole album at a time` : "Nothing to play in these albums");
  }

  /** Plays one of `albums`, picked at random. */
  async surpriseAlbum(albums: Album[]) {
    if (!albums.length) return;
    const id = await api.surpriseAlbum(albums.map((a) => a.id)).catch(() => null);
    const album = id ? this.albumById.get(id) : undefined;
    this.showToast(album ? `Playing “${album.title}” by ${album.artist}` : "Nothing to play in these albums");
  }

  togglePlay() {
    api.player("toggle");
  }

  playAlbum(album: Album, shuffle = false) {
    const tracks = this.albumTracks(album);
    if (shuffle) this.shuffleTracks(tracks);
    else this.playTracks(tracks);
  }

  openMenu(e: MouseEvent, items: MenuItem[]) {
    e.preventDefault();
    this.menu = { x: e.clientX, y: e.clientY, items };
  }

  /** The standard right-click menu for songs. */
  trackMenu(e: MouseEvent, tracks: Track[], startIndex = 0, extra: MenuItem[] = []) {
    const t = tracks[startIndex];
    this.openMenu(e, [
      ...extra,
      { label: "Play", icon: "play", action: () => this.playTracks(tracks, startIndex, true) },
      { label: "Play Next", icon: "playNext", action: () => this.enqueue([t], true) },
      { label: "Add to Queue", icon: "queue", action: () => this.enqueue([t], false) },
      { label: "Add to Playlist…", icon: "playlist", action: () => (this.picker = [t]) },
      this.isLoved(t)
        ? { label: "Remove from Favorites", icon: "heartFill", action: () => this.setLoved([t], false) }
        : { label: "Add to Favorites", icon: "heart", action: () => this.setLoved([t], true) },
      { label: "Rating", rating: { value: this.ratingOf(t), set: (n) => this.setRating([t], n) }, action: () => {} },
      { label: "Start Sound Radio", icon: "radio", action: () => this.soundRadio(t) },
      { label: "Songs That Sound Like This", icon: "wave", action: () => this.go({ name: "similar", id: t.id }) },
      { label: "Go to Album", icon: "album", action: () => this.go({ name: "album", id: t.albumId }) },
      ...(t.cue
        ? []
        : [
            { label: "Fix Song Info…", icon: "sparkle", action: () => (this.fixing = t) },
            { label: "Edit Info…", icon: "edit", action: () => (this.tagEditing = [t]) },
          ]),
      { label: "Change Cover…", icon: "album", action: () => (this.coverEditing = t.albumId) },
      { label: "Go to Artist", icon: "artist", action: () => this.go({ name: "artist", artist: this.artistOf(t.albumArtist) }) },
      this.skippedSet.has(t.id)
        ? { label: "Stop Skipping", icon: "skip", action: () => this.setSkipped([t], false) }
        : { label: "Skip This Song", icon: "skip", action: () => this.setSkipped([t], true) },
    ]);
  }

  /** The right-click menu for several selected songs (`tracks` in list order; a song listed twice
   *  plays and queues twice, but is rated, tagged and favourited once). */
  songsMenu(e: MouseEvent, tracks: Track[], extra: MenuItem[] = []) {
    if (!tracks.length) return;
    const unique = [...new Map(tracks.map((t) => [t.id, t])).values()];
    const loved = unique.every((t) => this.isLoved(t));
    const skipped = unique.every((t) => this.skippedSet.has(t.id));
    const r = this.ratingOf(unique[0]);
    const rating = unique.every((t) => this.ratingOf(t) === r) ? r : 0;
    const n = tracks.length;
    this.openMenu(e, [
      { label: `Play ${n} ${n === 1 ? "Song" : "Songs"}`, icon: "play", action: () => this.playTracks(tracks, 0, true) },
      ...extra,
      { label: "Play Next", icon: "playNext", action: () => this.enqueue(tracks, true) },
      { label: "Add to Queue", icon: "queue", action: () => this.enqueue(tracks, false) },
      { label: "Add to Playlist…", icon: "playlist", action: () => (this.picker = tracks) },
      loved
        ? { label: "Remove from Favorites", icon: "heartFill", action: () => this.setLoved(unique, false) }
        : { label: "Add to Favorites", icon: "heart", action: () => this.setLoved(unique, true) },
      { label: "Rating", rating: { value: rating, set: (stars) => this.setRating(unique, stars) }, action: () => {} },
      ...(unique.some((t) => t.cue) ? [] : [{ label: "Edit Info…", icon: "edit", action: () => (this.tagEditing = unique) }]),
      skipped
        ? { label: "Stop Skipping", icon: "skip", action: () => this.setSkipped(unique, false) }
        : { label: "Skip These Songs", icon: "skip", action: () => this.setSkipped(unique, true) },
    ]);
  }

  albumMenu(e: MouseEvent, album: Album) {
    this.openMenu(e, [
      { label: "Play", icon: "play", action: () => this.playAlbum(album) },
      { label: "Shuffle", icon: "shuffle", action: () => this.playAlbum(album, true) },
      { label: "Play Next", icon: "playNext", action: () => this.enqueue(this.albumTracks(album), true) },
      { label: "Add to Queue", icon: "queue", action: () => this.enqueue(this.albumTracks(album), false) },
      { label: "Add to Playlist…", icon: "playlist", action: () => (this.picker = this.albumTracks(album)) },
      ...(this.albumTracks(album).some((t) => t.cue)
        ? []
        : [{ label: "Edit Album Info…", icon: "edit", action: () => (this.tagEditing = this.albumTracks(album)) }]),
      ...(this.albumTracks(album).some((t) => t.cue)
        ? []
        : [{ label: "Fix Album Info…", icon: "sparkle", action: () => (this.fixingAlbum = album) }]),
      { label: "Change Cover…", icon: "album", action: () => (this.coverEditing = album.id) },
      { label: "Go to Artist", icon: "artist", action: () => this.go({ name: "artist", artist: this.artistOf(album.artist) }) },
    ]);
  }

  enqueue(tracks: Track[], next: boolean) {
    api.enqueue(tracks.map((t) => t.id), next);
    this.showToast(next ? `Playing next: ${label(tracks)}` : `Added to queue: ${label(tracks)}`);
  }

  async addFolders(paths: string[]) {
    if (paths.length === 0) return;
    this.startedAdding();
    try {
      this.folders = await api.addFolders(paths);
    } catch (e) {
      this.startedAdding(false);
      throw e;
    }
    this.showToast("Importing your music…");
  }

  /** Plays a song opened from another app ("Open with Cadence"). */
  async openFile(path: string) {
    try {
      const { id, added } = await api.openFile(path);
      this.opening = id;
      if (added) this.showToast(`Added “${path.split(/[\\/]/).pop()}” to your library`);
    } catch (e) {
      this.showToast(String(e));
    }
  }

  /** Asks for music folders and adds them. Android uses Cadence's own folder browser. */
  async pickFolders() {
    let picked: string[] = [];
    if (navigator.userAgent.includes("Android")) {
      // Cadence's own folder browser: the system picker hides or refuses music folders on some phones.
      const path = await new Promise<string | null>((resolve) => (this.folderBrowser = resolve));
      if (path) picked = [path];
    } else {
      const res = await open({ directory: true, multiple: true, title: "Add music folders" });
      if (res) picked = Array.isArray(res) ? res : [res];
    }
    if (picked.length) await this.addFolders(picked);
  }

  /** Android's Back button: closes what's on top, else goes back a page. False when there's nothing to go back to. */
  handleBack(): boolean {
    if (this.menu) this.menu = null;
    // Every dialog (the store's and the pages' own) is a Modal: the top-most one closes itself.
    else if (closeTopModal()) return true;
    else if (this.nowPlayingOpen) this.nowPlayingOpen = false;
    else if (this.queueOpen) this.queueOpen = false;
    // A song list in selection mode leaves it before the page goes back.
    else if (leaveSelection()) return true;
    else if (this.history.length > 0) this.back();
    else return false;
    return true;
  }

  // ---------- playlists ----------

  async savePlaylist(p: Playlist): Promise<Playlist> {
    const saved = await api.savePlaylist($state.snapshot(p) as Playlist);
    const i = this.playlists.findIndex((x) => x.id === saved.id);
    if (i >= 0) this.playlists[i] = saved;
    else this.playlists.push(saved);
    return saved;
  }

  async createPlaylist(name: string, tracks: Track[] = []): Promise<Playlist> {
    return this.savePlaylist({ id: "", name, trackIds: tracks.map((t) => t.id), smart: null, created: 0, updated: 0 });
  }

  /** `dupes`: "skip" leaves out songs already in the playlist; "add" puts them in again (another
   *  version, on purpose: the picker asks first). */
  async addToPlaylist(p: Playlist, tracks: Track[], dupes: "skip" | "add" = "skip") {
    const ids = [...new Set(tracks.map((t) => t.id).filter((id) => dupes === "add" || !p.trackIds.includes(id)))];
    // Nothing new: the playlist isn't saved (or moved up as just changed).
    if (!ids.length) {
      this.showToast(`Already in “${p.name}”`);
      return;
    }
    await this.savePlaylist({ ...p, trackIds: [...p.trackIds, ...ids] });
    const one = ids.length === 1 ? tracks.find((t) => t.id === ids[0]) : undefined;
    this.showToast(`Added ${one ? `“${one.title}”` : `${ids.length} songs`} to “${p.name}”`);
  }

  /** A new, empty playlist ("Playlist 3"), opened. */
  async newPlaylist() {
    try {
      const p = await this.createPlaylist(`Playlist ${this.playlists.filter((x) => !x.smart).length + 1}`);
      this.go({ name: "playlist", id: p.id });
    } catch (e) {
      this.showToast(`Couldn't make the playlist: ${e}`);
    }
  }

  async deletePlaylist(p: Playlist) {
    await api.deletePlaylist(p.id);
    this.playlists = this.playlists.filter((x) => x.id !== p.id);
    if (this.view.name === "playlist" && this.view.id === p.id) this.go({ name: "home" });
  }

  /** Changes sound settings; the engine is updated at most every 40 ms while sliders move. */
  updateDsp(change: (d: DspSettings) => void) {
    // The output device changed and its settings aren't here yet: an edit now would carry the
    // old device's settings into the new one's profile. It's dropped (the slider then shows the
    // new device's value).
    if (!this.dsp || this.dspStale) return;
    change(this.dsp);
    // The device generation shown when the first edit of this batch was made (not when the timer
    // fires): if the output moved on meanwhile, the engine drops the stale send.
    const gen = this.dspTimer === undefined ? this.status?.deviceGen : this.dspGen;
    this.dspGen = gen;
    clearTimeout(this.dspTimer);
    const profile = this.lastProfile;
    this.dspTimer = setTimeout(() => {
      this.dspTimer = undefined;
      // Belt and braces: never sent once another device's settings are in use.
      if (!this.dsp || this.dspStale || profile !== this.lastProfile) return;
      api.setDsp($state.snapshot(this.dsp), gen);
    }, 40);
  }
  private dspGen: number | undefined;
  private dspTimer: ReturnType<typeof setTimeout> | undefined;
  /** The device (or its profile) changed and its settings are being asked for. */
  private dspStale = false;
  /** Numbers the requests for the settings: only the newest answer is used. */
  private dspSeq = 0;
  /** A pointer is held down in the window (a slider may be mid-drag). */
  private pointerHeld = false;
  /** The new device's settings, waiting for the pointer to be let go. */
  private deferredDsp: DspSettings | null = null;

  /** The settings of the device now in use, asked for after a device or profile change. */
  private reloadDsp(retry = true) {
    const seq = ++this.dspSeq;
    api
      .dsp()
      .then((d) => {
        if (seq !== this.dspSeq) return;
        if (d) this.applyDeviceDsp(d);
        else this.dspStale = false;
      })
      .catch(() => {
        if (seq !== this.dspSeq) return;
        if (retry) this.reloadDsp(false);
        // Can't be had: editing carries on with what's shown (as before the device changed).
        else this.dspStale = false;
      });
  }

  /** Shows the new device's settings, once no slider is being dragged (they'd jump under the
   *  finger otherwise); until then edits are dropped (see `updateDsp`). */
  private applyDeviceDsp(d: DspSettings) {
    if (this.pointerHeld) {
      this.deferredDsp = d;
      return;
    }
    this.deferredDsp = null;
    this.dsp = d;
    this.dspStale = false;
  }

  private watchPointer() {
    if (typeof window === "undefined") return;
    const up = () => {
      this.pointerHeld = false;
      const d = this.deferredDsp;
      if (d) this.applyDeviceDsp(d);
    };
    window.addEventListener("pointerdown", () => (this.pointerHeld = true), true);
    window.addEventListener("pointerup", up, true);
    window.addEventListener("pointercancel", up, true);
    // Let go outside the window (the release may never reach it).
    window.addEventListener("blur", up);
  }

  /**
   * After a backup was restored: everything the window holds that a restore can change, asked
   * for again (the app has already put the restored settings to use). The pages' own copies
   * (Settings' toggles, the Sound page's options) follow `restored`.
   */
  async reloadAfterRestore() {
    // An edit waiting to be sent would undo the restored sound settings.
    clearTimeout(this.dspTimer);
    this.dspTimer = undefined;
    this.deferredDsp = null;
    const dspSeq = ++this.dspSeq;
    const get = <T,>(p: Promise<T>): Promise<T | undefined> => p.then((v) => v ?? undefined, () => undefined);
    const [lib, folders, status, dsp, history, playlists, marks, skips, ai, images, sleep, prefs] = await Promise.all([
      get(api.library()),
      get(api.folders()),
      get(api.status()),
      get(api.dsp()),
      get(api.history()),
      get(api.playlists()),
      get(api.marks()),
      get(api.skips()),
      get(api.aiSettings()),
      get(api.artistImages()),
      get(api.sleepTimer()),
      get(api.attentionState()),
    ]);
    if (lib && Array.isArray(lib.tracks) && Array.isArray(lib.albums)) this.setLibrary(lib);
    if (Array.isArray(folders)) this.folders = folders;
    if (history) this.listening = history;
    if (Array.isArray(playlists)) this.playlists = playlists;
    if (marks) this.marks = marks;
    if (skips) {
      this.skipped = Array.isArray(skips.ids) ? skips.ids : [];
      this.skipInstrumentals = !!skips.instrumentals;
    }
    if (ai) this.ai = ai;
    if (images) this.artistImages = images;
    this.sleepEndsAt = sleep ?? null;
    if (prefs) this.attentionPrefs = prefs;
    if (dsp && dspSeq === this.dspSeq) {
      this.dsp = dsp;
      this.dspStale = false;
    }
    // The queue, the position and the device (a device change here asks for its settings itself).
    if (status && Array.isArray(status.queue)) this.applyStatus(status);
    void this.identityRun();
    this.searchKeysOutdated();
    api.voiceStatus().then((v) => v && (this.voice = v)).catch(() => {});
    this.loadDuplicates().catch(() => {});
    this.loadHealth().catch(() => {});
    this.loadArtistPairsCount().catch(() => {});
    if (this.aiReady && !this.aiMixes.length) this.loadAiMixes(false);
    this.restored++;
  }
  /** Counts the backup restores (pages re-read their own copies of settings when it moves). */
  restored = $state(0);

  /**
   * Plays a list from song `start`, at `position` seconds into it. The app has no "start at"
   * for a list, so the seek waits for the player to say the song is playing (from the status
   * updates it sends anyway; given up after a few seconds): sent at once, it could land on the
   * song before, or be lost when the new one starts.
   */
  async playFrom(ids: string[], start: number, position: number) {
    const id = ids[start];
    if (!id) return;
    this.startSeek = position > 0 ? { id, position, until: Date.now() + 6000 } : null;
    try {
      await api.play(ids, start, true);
    } catch (e) {
      this.startSeek = null;
      throw e;
    }
  }
  private startSeek: { id: string; position: number; until: number } | null = null;
  /** A status update: the waiting seek of `playFrom`, once its song has started. */
  private seekOnStart(s: Status) {
    const w = this.startSeek;
    if (!w) return;
    if (Date.now() > w.until) {
      this.startSeek = null;
      return;
    }
    const id = s.index != null ? s.queue[s.index] : undefined;
    // The new play, just started (a song already playing past the start doesn't count).
    if (id !== w.id || s.position > 2 || !(s.duration > 0)) return;
    this.startSeek = null;
    this.seek(w.position);
  }

  /** Optimistic position update so the slider doesn't jump back while the engine seeks. */
  seek(position: number) {
    this.posBase = { pos: position, at: performance.now(), playing: this.status?.playing ?? false };
    this.position = position;
    api.seek(position);
  }
}

function label(tracks: Track[], quoted = false) {
  if (tracks.length !== 1) return `${tracks.length} songs`;
  return quoted ? `“${tracks[0].title}”` : tracks[0].title;
}

/** "Song (Instrumental)", "Song - Off Vocal", "Song (Karaoke Ver.)"… (matches the engine's rule). */
/**
 * Artist entries written several ways (one `ArtistGroup` of `groupOf`) merged into one: shown
 * under the spelling with most songs (ties: the order the app lists the spellings in), with every
 * album once and the other spellings in `also`.
 */
export function mergeSpellings(entries: Artist[], groupOf: Map<string, ArtistGroup>): Artist[] {
  if (!groupOf.size) return entries;
  const out: Artist[] = [];
  const byGroup = new Map<ArtistGroup, Artist[]>();
  for (const e of entries) {
    const g = groupOf.get(e.name.toLowerCase());
    if (!g) out.push(e);
    else byGroup.set(g, [...(byGroup.get(g) ?? []), e]);
  }
  for (const [g, list] of byGroup) {
    const rank = (n: string) => {
      const i = g.names.findIndex((x) => x.toLowerCase() === n.toLowerCase());
      return i < 0 ? Infinity : i;
    };
    const best = [...list].sort((a, b) => b.trackCount - a.trackCount || rank(a.name) - rank(b.name))[0];
    const seen = new Set<string>();
    const albums: Album[] = [];
    for (const e of list) for (const al of e.albums) if (!seen.has(al.id)) (seen.add(al.id), albums.push(al));
    const also: string[] = [];
    for (const n of g.names) {
      const k = n.toLowerCase();
      if (k !== best.name.toLowerCase() && !also.some((x) => x.toLowerCase() === k)) also.push(n);
    }
    out.push({ name: best.name, albums, trackCount: albums.reduce((s, a) => s + a.trackIds.length, 0), also });
  }
  return out;
}

export function isInstrumental(title: string): boolean {
  const t = title.toLowerCase();
  return ["instrumental", "off vocal", "off-vocal", "offvocal", "karaoke", "backing track", "(inst", "[inst", "- inst", "inst.)", "inst.]"].some((w) => t.includes(w));
}

export function shuffled<T>(list: T[]): T[] {
  const a = [...list];
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}

export const app = new AppState();

export interface SignalPath {
  /** "bitperfect": untouched to the device; "lossless": untouched by Cadence; "enhanced": Cadence processes it. */
  tier: "bitperfect" | "lossless" | "enhanced";
  label: string;
  steps: string[];
}

/** Plain-language description of everything between the file and the speakers. */
export function signalPath(s: Status | null): SignalPath | null {
  const o = s?.output;
  const src = s?.source;
  if (!s || !o?.device || !src) return null;
  const steps: string[] = [];
  if (o.leveling != null && o.leveling !== 0) steps.push(`Leveling ${db(o.leveling)}`);
  if (app.dsp?.vocalCut) steps.push(`Vocals ${Math.round((1 - app.dsp.vocalCut) * 100)}%`);
  if (o.tone) steps.push(o.tone.smart ? "Smart EQ" : "Loudness compensation");
  if (o.eq) steps.push(o.eqHeadroom < 0 ? `EQ (pre-amp ${db(o.eqHeadroom)})` : "EQ");
  if (o.crossfeed) steps.push("Crossfeed");
  if (o.remixing) steps.push(`${src.channels} → ${o.channels} channels`);
  if (o.resampling) steps.push(`${khz(src.sampleRate)} → ${khz(o.sampleRate)}`);
  if (s.volume < 1) steps.push(`Volume ${Math.round(Math.sqrt(s.volume) * 100)}%`);
  if (o.deviceVolumeDb != null) steps.push(`DAC volume ${db(o.deviceVolumeDb)}`);
  if (o.bitPerfect) return { tier: "bitperfect", label: "Bit-perfect", steps };
  const mixer = systemMixer[0].toUpperCase() + systemMixer.slice(1);
  if (o.unaltered) return { tier: "lossless", label: "Lossless", steps: ["Unaltered by Cadence", ...(o.mode === "shared" ? [mixer] : [])] };
  return { tier: "enhanced", label: "Enhanced", steps: [...steps, ...(o.mode === "shared" ? [mixer] : [])] };
}

import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { flushSync } from "svelte";

/** Top-level places (tabs and sidebar): switching between them crossfades instead of sliding. */
const TOP_LEVEL = new Set(["home", "albums", "artists", "songs", "recent", "search", "sound", "settings"]);

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

/** Runs a view change inside a view transition (see app.css), when the system allows motion. */
function animate(kind: "forward" | "back" | "tab", change: () => void) {
  const doc = document as Document & { startViewTransition?: (cb: () => void) => unknown };
  // Phones skip view transitions: snapshotting both pages stalls a frame for ~50 ms there. The
  // new page animates in by itself instead (.page-in in app.css), which costs nothing.
  if (!doc.startViewTransition || matchMedia("(prefers-reduced-motion: reduce)").matches || document.documentElement.classList.contains("phone")) {
    change();
    return;
  }
  const root = document.documentElement;
  root.dataset.nav = kind;
  const run = ++transitions;
  const t = doc.startViewTransition(() => flushSync(change)) as { finished?: Promise<void> } | undefined;
  // The page's transition names (app.css) are only needed while it animates.
  t?.finished?.finally(() => {
    if (run === transitions) delete root.dataset.nav;
  });
}
import { api, artChanged, artistIndex, db, khz, type Album, type AiMix, type AiSettings, type DspSettings, type HistoryStats, type Mix, type Playlist, type SonicStatus, type Snapshot, type Status, type Track } from "./api";

export type View =
  | { name: "home" }
  | { name: "albums" }
  | { name: "album"; id: string }
  | { name: "artists" }
  | { name: "artist"; artist: string }
  | { name: "songs" }
  | { name: "recent" }
  | { name: "search"; query: string }
  | { name: "sound" }
  | { name: "playlist"; id: string }
  | { name: "similar"; id: string }
  | { name: "settings" };

export interface MenuItem {
  label: string;
  icon?: string;
  action: () => void;
}

export interface Artist {
  name: string;
  albums: Album[];
  trackCount: number;
}

class AppState {
  tracks = $state<Track[]>([]);
  albums = $state<Album[]>([]);
  /** Replaced whole on every update (four times a second while playing), never changed in place. */
  status = $state.raw<Status | null>(null);
  folders = $state<string[]>([]);
  scan = $state<{ done: number; total: number } | null>(null);
  loaded = $state(false);

  view = $state<View>({ name: "home" });
  /** Counts page changes, and how the last one went, for the phone's page animation. */
  navSeq = $state(0);
  navKind = $state<"forward" | "back" | "tab">("tab");
  history = $state<View[]>([]);
  nowPlayingOpen = $state(false);
  /** A song opened from another app: Now Playing opens when it starts. */
  opening = $state<string | null>(null);
  lyricsOpen = $state(false);
  queueOpen = $state(false);
  search = $state("");
  toast = $state<string | null>(null);
  dsp = $state<DspSettings | null>(null);
  analysis = $state<{ done: number; total: number } | null>(null);
  /** How much of the library has had sound analysis ("sounds like"). */
  sonic = $state<SonicStatus | null>(null);
  /** AI DJ settings, and whether an API key is saved. */
  ai = $state<{ settings: AiSettings; hasKey: boolean } | null>(null);
  aiReady = $derived(!!this.ai && this.ai.settings.provider === "cloud" && this.ai.hasKey);
  /** Today's AI mixes, and whether they're being made or failed. */
  aiMixes = $state<AiMix[]>([]);
  aiMixesState = $state<"idle" | "loading" | "error">("idle");
  /** Today's new-music picks (songs not in the library). */
  aiNew = $state<import("./api").NewSong[]>([]);
  aiNewState = $state<"idle" | "loading" | "error">("idle");
  aiNewError = $state("");
  aiMixesError = $state("");
  /** Mixes of songs that sound alike. */
  mixes = $state<Mix[]>([]);
  menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);
  playlists = $state<Playlist[]>([]);
  /** Songs waiting for the "Add to playlist" picker. */
  picker = $state<Track[] | null>(null);
  /** Songs whose info (tags) is being edited. */
  tagEditing = $state<Track[] | null>(null);
  /** Pictures chosen for artists, by lower-case name. */
  artistImages = $state<Record<string, import("./api").ArtistImage>>({});
  /** Settings > Appearance. */
  theme = $state<Theme>(savedTheme());

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
  /** The open folder browser (Android): call with the chosen folder, or null if cancelled. */
  folderBrowser = $state<((path: string | null) => void) | null>(null);
  /** The smart playlist being edited (a copy), or null. */
  smartEditing = $state<Playlist | null>(null);

  trackById = $derived(new Map(this.tracks.map((t) => [t.id, t])));
  albumById = $derived(new Map(this.albums.map((a) => [a.id, a])));
  /** Album-artist credit → the artists it's listed under (see `artistIndex`). */
  creditIndex = $derived(artistIndex(this.albums.map((a) => a.artist)));

  /** One entry per artist; albums credited to "A & B" appear under each real artist. */
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
    return [...map.values()].sort((x, y) => x.name.localeCompare(y.name));
  });

  /** The artist page for a credit: its lead artist. */
  artistOf(credit: string): string {
    return this.creditIndex.get(credit)?.[0] ?? credit;
  }

  current = $derived.by(() => {
    const s = this.status;
    if (!s || s.index == null) return null;
    return this.trackById.get(s.queue[s.index]) ?? null;
  });
  currentAlbum = $derived(this.current ? (this.albumById.get(this.current.albumId) ?? null) : null);

  /** The album whose colours theme the app: the one playing, else the one on screen. */
  themeAlbum = $derived.by(() => this.currentAlbum ?? (this.view.name === "album" ? (this.albumById.get(this.view.id) ?? null) : null));

  /** Listening history stats (plays, skips, recent albums). */
  listening = $state<HistoryStats | null>(null);
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

  /** An artist's picture: the one chosen for them, else their latest album's cover. */
  artistPicture(artist: Artist, size: "large" | "small"): string | null {
    const own = this.artistImages[artist.name.toLowerCase()];
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
    return t.artist === "Unknown Artist";
  }

  /** Whether `t` is left out when a list plays (same rule as the engine's). */
  isSkipped(t: Track): boolean {
    return this.skippedSet.has(t.id) || (this.skipInstrumentals && isInstrumental(t.title));
  }

  async setSkipped(tracks: Track[], skip: boolean) {
    const ids = tracks.map((t) => t.id);
    this.skipped = skip ? [...new Set([...this.skipped, ...ids])] : this.skipped.filter((id) => !ids.includes(id));
    await api.setSkipped(ids, skip);
    this.showToast(skip ? `Skipping ${label(tracks, true)} from now on` : `No longer skipping ${label(tracks, true)}`);
  }

  setSkipInstrumentals(on: boolean) {
    this.skipInstrumentals = on;
    api.setSkipInstrumentals(on);
  }

  /**
   * Where playback is, as of the player's last update (four times a second while playing). Not
   * every frame: that made the window redraw up to 160 times a second (about 15% of a core) and
   * left garbage behind. Seek lines glide between updates with CSS; what has to move smoothly
   * (the karaoke sweep) uses `livePosition()`.
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
      listen<Omit<Status, "queue"> & { queue?: string[] }>("player-status", (e) => {
        const queue = e.payload.queue ?? this.status?.queue;
        if (queue) this.applyStatus({ ...e.payload, queue });
      }),
      listen("library-changed", async () => {
        await this.loadLibrary();
        this.scan = null;
      }),
      listen<{ done: number; total: number }>("library-progress", (e) => (this.scan = e.payload)),
      listen("history-changed", async () => (this.listening = await api.history())),
      listen<{ done: number; total: number }>("analysis-progress", (e) => {
        this.analysis = e.payload.done >= e.payload.total ? null : e.payload;
      }),
      listen<{ done: number; total: number }>("sonic-progress", (e) => {
        const { done, total } = e.payload;
        if (done >= total) this.loadSonic();
        else if (this.sonic) this.sonic = { ...this.sonic, analyzed: this.sonic.total - total + done, running: true };
      }),
    ]);
    this.loadSonic();
    api.aiSettings().then((a) => {
      this.ai = a;
      if (this.aiReady) {
        this.loadAiMixes(false);
        this.loadAiNew(false);
      }
    });
    api.artistImages().then((m) => (this.artistImages = m));
    // The mini player is another window: follow a theme changed in the main one.
    window.addEventListener("storage", (e) => e.key === "cadence-theme" && (this.theme = savedTheme()));
    api.skips().then((s) => {
      this.skipped = s.ids;
      this.skipInstrumentals = s.instrumentals;
    });
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
    this.folders = folders;
    // An update that came in meanwhile is newer.
    if (!this.status) this.applyStatus(status);
    this.dsp = dsp;
    this.loaded = true;
  }

  private applyStatus(s: Status) {
    this.status = s;
    const albumId = s.index != null ? this.trackById.get(s.queue[s.index])?.albumId : undefined;
    if (albumId && this.sessionAlbums[0] !== albumId) this.sessionAlbums = [albumId, ...this.sessionAlbums.filter((a) => a !== albumId)].slice(0, 20);
    this.posBase = { pos: s.position, at: performance.now(), playing: s.playing };
    this.position = this.exactPosition();
    if (s.error && s.error !== this.lastError) this.showToast(s.error);
    this.lastError = s.error;
  }
  private lastError: string | null = null;

  async loadLibrary() {
    this.setLibrary(await api.library());
  }

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

  async loadAiNew(refresh: boolean) {
    if (!this.aiReady || this.aiNewState === "loading") return;
    this.aiNewState = "loading";
    try {
      this.aiNew = await api.aiDiscover(refresh);
      this.aiNewState = "idle";
    } catch (e) {
      this.aiNewError = String(e);
      this.aiNewState = "error";
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

  go(view: View) {
    // Typing in the search box updates the same page: no animation for that.
    if (view.name === "search" && this.view.name === "search") {
      this.view = view;
      return;
    }
    const kind = TOP_LEVEL.has(view.name) ? "tab" : "forward";
    animate(kind, () => {
      this.history.push(this.view);
      this.view = view;
      this.navKind = kind;
      this.navSeq++;
      this.nowPlayingOpen = false;
      document.querySelector(".content")?.scrollTo({ top: 0 });
    });
  }

  back() {
    const prev = this.history.pop();
    if (prev)
      animate("back", () => {
        this.view = prev;
        this.navKind = "back";
        this.navSeq++;
      });
  }

  showToast(message: string) {
    this.toast = message;
    clearTimeout(this.toastTimer);
    this.toastTimer = setTimeout(() => (this.toast = null), 4000);
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

  togglePlay() {
    api.player("toggle");
  }

  playAlbum(album: Album, shuffle = false) {
    let tracks = this.albumTracks(album);
    if (shuffle) tracks = shuffled(tracks);
    this.playTracks(tracks);
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
      { label: "Start Sound Radio", icon: "radio", action: () => this.soundRadio(t) },
      { label: "Songs That Sound Like This", icon: "wave", action: () => this.go({ name: "similar", id: t.id }) },
      { label: "Go to Album", icon: "album", action: () => this.go({ name: "album", id: t.albumId }) },
      { label: "Fix Song Info…", icon: "sparkle", action: () => (this.fixing = t) },
      { label: "Edit Info…", icon: "edit", action: () => (this.tagEditing = [t]) },
      { label: "Change Cover…", icon: "album", action: () => (this.coverEditing = t.albumId) },
      { label: "Go to Artist", icon: "artist", action: () => this.go({ name: "artist", artist: this.artistOf(t.albumArtist) }) },
      this.skippedSet.has(t.id)
        ? { label: "Stop Skipping", icon: "skip", action: () => this.setSkipped([t], false) }
        : { label: "Skip This Song", icon: "skip", action: () => this.setSkipped([t], true) },
    ]);
  }

  albumMenu(e: MouseEvent, album: Album) {
    this.openMenu(e, [
      { label: "Play", icon: "play", action: () => this.playAlbum(album) },
      { label: "Shuffle", icon: "shuffle", action: () => this.playAlbum(album, true) },
      { label: "Play Next", icon: "playNext", action: () => this.enqueue(this.albumTracks(album), true) },
      { label: "Add to Queue", icon: "queue", action: () => this.enqueue(this.albumTracks(album), false) },
      { label: "Add to Playlist…", icon: "playlist", action: () => (this.picker = this.albumTracks(album)) },
      { label: "Edit Album Info…", icon: "edit", action: () => (this.tagEditing = this.albumTracks(album)) },
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
    this.folders = await api.addFolders(paths);
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
    else if (this.picker) this.picker = null;
    else if (this.tagEditing) this.tagEditing = null;
    else if (this.fixing) this.fixing = null;
    else if (this.artistEditing) this.artistEditing = null;
    else if (this.skippedOpen) this.skippedOpen = false;
    else if (this.coverEditing) this.coverEditing = null;
    else if (this.smartEditing) this.smartEditing = null;
    else if (this.folderBrowser) {
      this.folderBrowser(null);
      this.folderBrowser = null;
    }
    else if (this.nowPlayingOpen) this.nowPlayingOpen = false;
    else if (this.queueOpen) this.queueOpen = false;
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

  async addToPlaylist(p: Playlist, tracks: Track[]) {
    const ids = tracks.map((t) => t.id).filter((id) => !p.trackIds.includes(id));
    await this.savePlaylist({ ...p, trackIds: [...p.trackIds, ...ids] });
    this.showToast(ids.length ? `Added ${ids.length === 1 ? `“${tracks[0].title}”` : `${ids.length} songs`} to “${p.name}”` : `Already in “${p.name}”`);
  }

  async deletePlaylist(p: Playlist) {
    await api.deletePlaylist(p.id);
    this.playlists = this.playlists.filter((x) => x.id !== p.id);
    if (this.view.name === "playlist" && this.view.id === p.id) this.go({ name: "home" });
  }

  /** Changes sound settings; the engine is updated at most every 40 ms while sliders move. */
  updateDsp(change: (d: DspSettings) => void) {
    if (!this.dsp) return;
    change(this.dsp);
    clearTimeout(this.dspTimer);
    this.dspTimer = setTimeout(() => api.setDsp($state.snapshot(this.dsp!)), 40);
  }
  private dspTimer: ReturnType<typeof setTimeout> | undefined;

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

/**
 * The position 30 times a second while playing, for what has to move smoothly and can't glide
 * with CSS: the karaoke sweep. A timer, not `requestAnimationFrame`: a callback on every frame
 * (160 a second on some screens) makes the window redraw that often even when nothing changes,
 * about 15% of a processor core. Runs only while the component that called it (during its setup)
 * is shown.
 */
export function livePosition(): { readonly value: number } {
  let value = $state(app.position);
  const playing = $derived(!!app.status?.playing);
  $effect(() => {
    if (!playing) {
      value = app.position;
      return;
    }
    const timer = setInterval(() => (value = app.exactPosition()), 33);
    return () => clearInterval(timer);
  });
  return {
    get value() {
      return value;
    },
  };
}

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

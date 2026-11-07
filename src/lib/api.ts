import { convertFileSrc, invoke } from "@tauri-apps/api/core";

export interface Track {
  id: string;
  path: string;
  title: string;
  artist: string;
  album: string;
  albumArtist: string;
  albumId: string;
  genre: string;
  /** The subtitle tag: Fix Song Info keeps the catalogue's title there when a song keeps its own. */
  subtitle?: string;
  year: number | null;
  trackNo: number | null;
  discNo: number | null;
  duration: number;
  sampleRate: number | null;
  bitDepth: number | null;
  channels: number | null;
  bitrate: number | null;
  codec: string;
  /** File size in bytes. */
  size: number;
  /** A song of a single-file album (CUE sheet): where it sits in `path`, the whole album's file. */
  cue?: { start: number; end: number | null };
  added: number;
  gain: SongGain;
}

/** A song's favourite heart and star rating (0 = not rated). */
export interface Mark {
  rating: number;
  loved: boolean;
}

export interface DupSong {
  id: string;
  /** The copy to keep. */
  keep: boolean;
}
export interface DupGroup {
  /** "same": the same song twice on one album. "lossy": a lossy copy of a song that's also lossless. */
  kind: "same" | "lossy";
  songs: DupSong[];
  wasted: number;
}
export interface HealthQualityRow {
  id: string;
  verdict: "good" | "suspect" | "lossy" | "upsampled";
  cutoffHz: number | null;
  realBits: number | null;
}
export interface HealthIntegrityRow {
  id: string;
  verified: boolean | null;
  errors: number;
  truncated: boolean;
  failed: string | null;
}
export type HealthKind = "quality" | "integrity";
export interface HealthReport {
  running: HealthKind | null;
  songs: number;
  lossless: number;
  qualityChecked: number;
  integrityChecked: number;
  quality: HealthQualityRow[];
  integrity: HealthIntegrityRow[];
}
export interface HealthProgress {
  kind: HealthKind;
  done: number;
  total: number;
  running: boolean;
}
export interface Removed {
  deleted: number;
  freed: number;
  skipped: string[];
}

export interface GainInfo {
  gain: number;
  peak: number;
}

export interface SongGain {
  track: GainInfo | null;
  album: GainInfo | null;
}

export interface Album {
  id: string;
  title: string;
  artist: string;
  year: number | null;
  genre: string;
  art: string | null;
  artSmall: string | null;
  trackIds: string[];
  duration: number;
  quality: string;
  format: string;
  added: number;
  /** Readable accent colour from the cover (hex). */
  accent: string | null;
  /** Deep background tint from the cover (hex). */
  tint: string | null;
}

export interface Snapshot {
  tracks: Track[];
  albums: Album[];
}

export type Repeat = "off" | "all" | "one";

export interface SourceInfo {
  codec: string;
  sampleRate: number;
  bits: number | null;
  channels: number;
}

export interface OutputInfo {
  device: string;
  sampleRate: number;
  channels: number;
  mode: "shared" | "exclusive";
  resampling: boolean;
  eq: boolean;
  eqHeadroom: number;
  crossfeed: boolean;
  /** Loudness gain on the current song (dB), null when leveling is off. */
  leveling: number | null;
  /** Per-song tone on the current song: loudness compensation shelves and Smart EQ curve. */
  tone: { bass: number; treble: number; smart: number[] | null } | null;
  remixing: boolean;
  /** Cadence passes samples through unchanged. */
  unaltered: boolean;
  /** Unaltered and sent straight to the device (exclusive mode). */
  bitPerfect: boolean;
  /** Integer bits sent to the device (exclusive mode). */
  bits: number | null;
  /** The DAC's own volume Cadence applies (dB) when below full: Android's USB exclusive, where the volume keys move it. */
  deviceVolumeDb: number | null;
  /** E.g. exclusive mode fell back to shared. */
  note: string | null;
  peakDb: number;
  clipping: boolean;
}

export interface Status {
  queue: string[];
  index: number | null;
  playing: boolean;
  position: number;
  duration: number;
  volume: number;
  repeat: Repeat;
  shuffle: boolean;
  source: SourceInfo | null;
  output: OutputInfo;
  error: string | null;
  /** The sleep timer's song or album rule, while one is set. */
  stopAfter: { kind: "songs"; left: number } | { kind: "album" } | null;
}

export type FilterKind = "peak" | "lowshelf" | "highshelf" | "lowpass" | "highpass";

export interface Filter {
  kind: FilterKind;
  freq: number;
  gain: number;
  q: number;
  enabled: boolean;
}

export interface EqSettings {
  enabled: boolean;
  mode: "graphic" | "parametric";
  preamp: number;
  gains: number[];
  preset: string;
  filters: Filter[];
  profile: string;
}

export type LevelingMode = "off" | "track" | "album" | "auto";

export interface DspSettings {
  eq: EqSettings;
  leveling: { mode: LevelingMode; preamp: number };
  crossfeed: { enabled: boolean; level: "default" | "cmoy" | "meier" };
  /** WASAPI exclusive mode. */
  exclusive: boolean;
  crossfade: { enabled: boolean; seconds: number };
  /** Loudness compensation and Smart EQ, per song. */
  tone: { loudness: boolean; smart: boolean };
  /** Sing along: how far the lead vocal is turned down (0–1). */
  vocalCut: number;
}

export interface Preset {
  name: string;
  gains: number[];
}

export interface HistoryStats {
  plays: Record<string, number>;
  skips: Record<string, number>;
  lastPlayed: Record<string, number>;
  recentAlbums: string[];
  totalListened: number;
}

export interface LyricLine {
  time: number;
  text: string;
  /** Word timings ("enhanced" LRC, or placed on the singing), when the lyrics have them. */
  words?: { time: number; text: string; end?: number }[];
  /** Who sings the line: indexes into `Lyrics.singers` (several when they sing it together). */
  singers?: number[];
}

export interface Lyrics {
  source: "embedded" | "file" | "lrclib" | "pasted" | "ai" | "";
  synced: LyricLine[];
  plain: string;
  instrumental: boolean;
  /** Why nothing was found online: "off", "missing", "unavailable", or "" when found. */
  lookup: "off" | "missing" | "unavailable" | "hidden" | "";
  /** Placed on the singing: "on" (these lines), "off" (turned off for this song), absent if not yet. */
  voice?: "on" | "off";
  /** The singers, when the song has several and their lines are known; "" when told apart by voice but not named. */
  singers?: string[];
  /** Where the singers come from: named in the lyrics, found by the AI, or told apart by their voices. */
  singersFrom?: "lyrics" | "ai" | "voice";
}

/** Following the singer: Cadence listens to a song once and places every word on the singing. */
export interface VoiceJob {
  trackId: string;
  title: string;
  done: number;
  total: number;
}

export interface VoiceStatus {
  /** The models can run on this system. */
  supported: boolean;
  installed: boolean;
  /** Bytes downloaded and in all, while downloading. */
  installing: [number, number] | null;
  downloadSize: number;
  /** Why the last download failed, or "". */
  error: string;
  /** Follow the singer for every song whose lyrics are opened. */
  auto: boolean;
  job: VoiceJob | null;
  /** Following the singer in every song with lyrics (or how the last such run ended). */
  batch: VoiceBatch | null;
}

export interface VoiceBatch {
  running: boolean;
  /** Still finding the songs with lyrics. */
  preparing: boolean;
  done: number;
  total: number;
  /** Songs whose lyrics couldn't be found in the singing. */
  failed: number;
  secondsLeft: number | null;
}

export interface Playlist {
  id: string;
  name: string;
  trackIds: string[];
  /** Smart playlist rules (see smart.ts); null for hand-made playlists. */
  smart: import("./smart").Smart | null;
  created: number;
  updated: number;
}

/** A group of songs that sound alike (from local sound analysis). */
export interface Mix {
  name: string;
  description: string;
  trackIds: string[];
}

export interface SonicStatus {
  analyzed: number;
  total: number;
  running: boolean;
}

export type AiProvider = "off" | "cloud";

/** Claude (Anthropic) or any OpenAI-compatible service (Z.ai GLM by default). */
export type AiService = "claude" | "openai";

export interface AiSettings {
  provider: AiProvider;
  service: AiService;
  cloudUrl: string;
  cloudModel: string;
  claudeModel: string;
  /** Let the model search the web (new music, lyrics). */
  webSearch: boolean;
}

/** An LRCLIB search result. */
export interface LyricsHit {
  id: number;
  title: string;
  artist: string;
  album: string;
  duration: number;
  instrumental: boolean;
  synced: boolean;
  preview: string;
}

/** A folder in the in-app folder browser. */
export interface DirEntry {
  name: string;
  path: string;
  /** Music files directly inside. */
  songs: number;
  hasFolders: boolean;
}

/** A song suggested by the DJ that isn't in the library. */
export interface NewSong {
  artist: string;
  title: string;
  why: string;
  album?: string;
  /** Album cover, or the artist's picture when `imageIsArtist`; empty if unknown. */
  image?: string;
  imageIsArtist?: boolean;
  /** Discover: how close it is to what was asked for, 0–100. */
  similarity?: number;
  /** Discover: how well listeners and critics rate it, 0–100. */
  rating?: number;
  /** Discover: the song ("Artist - Title") or artist in the library it's most like. */
  like?: string;
}

/** How often an artist, genre or song was played. */
export interface PlayCount {
  name: string;
  plays: number;
}

/** What the listener plays: the last 30 days ("this month") and all time. */
export interface ListeningProfile {
  monthArtists: PlayCount[];
  monthGenres: PlayCount[];
  monthSongs: PlayCount[];
  artists: PlayCount[];
  genres: PlayCount[];
}

export type DiscoverBasis = "month" | "artist" | "genre" | "search";

/** A song, artist or album in the listening stats. */
export interface StatItem {
  /** Track or album id; the name for an artist. */
  id: string;
  name: string;
  sub: string;
  /** Plays heard to the end. */
  finished: number;
  plays: number;
  minutes: number;
}

export type StatsRange = "week" | "month" | "year" | "all";

export interface ListeningStats {
  range: StatsRange;
  plays: number;
  finished: number;
  skipped: number;
  minutes: number;
  songs: number;
  artists: number;
  albums: number;
  topSongs: StatItem[];
  topArtists: StatItem[];
  topAlbums: StatItem[];
  /** Minutes per day (week, month) or per 30 days (year), oldest first. */
  timeline: number[];
}

/** A Discover search and what it found. */
export interface Recommendations {
  basis: DiscoverBasis | "";
  value: string;
  at: number;
  picks: NewSong[];
}

/** Stores that sell downloads (lossless where they can), searched for a song. */
export const STORES: { name: string; url: (q: string) => string }[] = [
  { name: "Qobuz", url: (q) => `https://www.qobuz.com/us-en/search?q=${q}` },
  { name: "Bandcamp", url: (q) => `https://bandcamp.com/search?q=${q}&item_type=t` },
  { name: "OTOTOY", url: (q) => `https://ototoy.jp/find/?q=${q}` },
  { name: "mora", url: (q) => `https://mora.jp/search/top?keyWord=${q}` },
  { name: "iTunes", url: (q) => `https://music.apple.com/search?term=${q}` },
  { name: "Amazon", url: (q) => `https://www.amazon.com/s?k=${q}&i=digital-music` },
];

/** Songs the AI DJ picked. */
export interface AiMix {
  title: string;
  reason: string;
  trackIds: string[];
  /** Songs not in the library that fit. */
  discover?: NewSong[];
}

export const EQ_BANDS = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

export const api = {
  library: () => invoke<Snapshot>("get_library"),
  folders: () => invoke<string[]>("get_folders"),
  addFolders: (paths: string[]) => invoke<string[]>("add_folders", { paths }),
  removeFolder: (path: string) => invoke<string[]>("remove_folder", { path }),
  rescan: () => invoke<void>("rescan"),
  /** The page is changing: background listening gives way for a moment. */
  uiBusy: () => invoke<void>("ui_busy"),
  listDirs: (path: string | null, volumes?: { path: string; name: string }[]) =>
    invoke<DirEntry[]>("list_dirs", { path, volumes }),
  openFile: (path: string) => invoke<{ id: string; added: boolean }>("open_file", { path }),
  /** `picked`: the user chose the start song, so it plays even if it's skipped. */
  play: (ids: string[], start = 0, picked = false) => invoke<void>("play_tracks", { ids, start, picked }),
  enqueue: (ids: string[], next: boolean) => invoke<void>("enqueue", { ids, next }),
  player: (action: "play" | "pause" | "toggle" | "next" | "prev") => invoke<void>("player", { action }),
  seek: (position: number) => invoke<void>("seek", { position }),
  jump: (index: number) => invoke<void>("jump", { index }),
  removeFromQueue: (index: number) => invoke<void>("remove_from_queue", { index }),
  clearUpNext: () => invoke<void>("clear_up_next"),
  moveInQueue: (from: number, to: number) => invoke<void>("move_in_queue", { from, to }),
  insertInQueue: (at: number, ids: string[]) => invoke<void>("insert_in_queue", { at, ids }),
  setSleepTimer: (kind: "off" | "minutes" | "songs" | "album", amount?: number) => invoke<void>("set_sleep_timer", { kind, amount: amount ?? null }),
  sleepTimer: () => invoke<number | null>("get_sleep_timer"),
  setVolume: (volume: number) => invoke<void>("set_volume", { volume }),
  setRepeat: (repeat: Repeat) => invoke<void>("set_repeat", { repeat }),
  /** Shuffle the songs coming up (and every list played from now on); off puts them back in order. */
  setShuffle: (on: boolean) => invoke<void>("set_shuffle", { on }),
  status: () => invoke<Status>("get_status"),
  dsp: () => invoke<DspSettings>("get_dsp"),
  setDsp: (dsp: DspSettings) => invoke<void>("set_dsp", { dsp }),
  eqPresets: () => invoke<Preset[]>("eq_presets"),
  parseEqFile: (args: { text?: string; path?: string }) => invoke<{ preamp: number; filters: Filter[] }>("parse_eq_file", args),
  reanalyze: () => invoke<void>("reanalyze"),
  history: () => invoke<HistoryStats>("get_history"),
  appSettings: () => invoke<{ closeToTray: boolean; globalHotkeys: boolean; lyricsOnline: boolean; fixTagsOnline: boolean; listenbrainzUser: string }>("get_app_settings"),
  connectListenBrainz: (token: string) => invoke<string>("connect_listenbrainz", { token }),
  disconnectListenBrainz: () => invoke<void>("disconnect_listenbrainz"),
  setLyricsOnline: (enabled: boolean) => invoke<void>("set_lyrics_online", { enabled }),
  findAlbumInfo: (albumId: string, query?: string) =>
    invoke<{ term: string; results: AlbumMatch[] }>("find_album_info", { albumId, query: query ?? null }),
  albumPlan: (albumId: string, albumRef: string) => invoke<AlbumPlan>("album_plan", { albumId, albumRef }),
  saveAlbumInfo: (albumId: string, album: AlbumMatch, songs: CatalogSong[], assign: SongAssign[], keepTitles: boolean) =>
    invoke<{ songs: number; matched: number; later: boolean; change: string | null }>("save_album_info", { albumId, album, songs, assign, keepTitles }),
  fixAllStart: () => invoke<boolean>("fix_all_start"),
  fixAllStop: () => invoke<void>("fix_all_stop"),
  /** What Fix all found, to check before saving. */
  fixAllFound: () => invoke<FoundInfo[]>("fix_all_found"),
  fixAllSave: (ids: string[]) => invoke<{ songs: number; change: string | null }>("fix_all_save", { ids }),
  fixAllDiscard: () => invoke<void>("fix_all_discard"),
  /** Cadence's recent changes to songs' files, newest first. */
  recentChanges: () => invoke<ChangeInfo[]>("recent_changes"),
  /** Puts a change's files back as they were; returns how many songs. */
  undoChange: (id: string) => invoke<number>("undo_change", { id }),
  artistCheckStart: () => invoke<boolean>("artist_check_start"),
  artistCheckStop: () => invoke<void>("artist_check_stop"),
  artistPairs: () => invoke<ArtistPairs>("artist_pairs"),
  artistPairDismiss: (native: string, latin: string) => invoke<void>("artist_pair_dismiss", { native, latin }),
  /** Renames an artist in every song's artist and album artist; returns how many songs. */
  mergeArtist: (from: string, to: string) => invoke<{ songs: number; change: string | null }>("merge_artist", { from, to }),
  findSongInfo: (id: string, query?: string) => invoke<{ term: string; results: SongInfo[] }>("find_song_info", { id, query: query ?? null }),
  /** keepTitle: the song's own title instead of the match's; wholeAlbum: the album's info for its other songs too. */
  saveSongInfo: (id: string, info: SongInfo, keepTitle: boolean, wholeAlbum: boolean) =>
    invoke<SongInfoSaved>("save_song_info", { id, info, keepTitle, wholeAlbum }),
  findCovers: (albumId: string, query?: string) => invoke<{ term: string; results: CoverResult[] }>("find_covers", { albumId, query: query ?? null }),
  setAlbumCover: (albumId: string, source: { kind: "url"; url: string } | { kind: "file"; path: string }) =>
    invoke<{ art: string; artSmall: string; later: number; failed: number }>("set_album_cover", { albumId, source }),
  artistImages: () => invoke<Record<string, ArtistImage>>("artist_images"),
  findArtistPhotos: (name: string) => invoke<ArtistPhoto[]>("find_artist_photos", { name }),
  setArtistImage: (name: string, source: { kind: "url"; url: string } | { kind: "file"; path: string }) =>
    invoke<ArtistImage>("set_artist_image", { name, source }),
  resetArtistImage: (name: string) => invoke<void>("reset_artist_image", { name }),
  skips: () => invoke<{ ids: string[]; instrumentals: boolean }>("get_skips"),
  setSkipped: (ids: string[], skip: boolean) => invoke<void>("set_skipped", { ids, skip }),
  setSkipInstrumentals: (enabled: boolean) => invoke<void>("set_skip_instrumentals", { enabled }),
  marks: () => invoke<Record<string, Mark>>("get_marks"),
  setRating: (ids: string[], rating: number) => invoke<void>("set_rating", { ids, rating }),
  setLoved: (ids: string[], loved: boolean) => invoke<void>("set_loved", { ids, loved }),
  setFixTagsOnline: (enabled: boolean) => invoke<void>("set_fix_tags_online", { enabled }),
  lyrics: (trackId: string) => invoke<Lyrics>("get_lyrics", { trackId }),
  voiceStatus: () => invoke<VoiceStatus>("voice_status"),
  voiceInstall: () => invoke<void>("voice_install"),
  voiceRemove: () => invoke<void>("voice_remove"),
  voiceSetAuto: (enabled: boolean) => invoke<void>("voice_set_auto", { enabled }),
  voiceFollow: (trackId: string) => invoke<void>("voice_follow", { trackId }),
  voiceUse: (trackId: string, on: boolean) => invoke<Lyrics>("voice_use", { trackId, on }),
  voiceBatch: () => invoke<void>("voice_batch"),
  voiceBatchStop: () => invoke<void>("voice_batch_stop"),
  /** Lines in Latin letters (romaji, pinyin, Korean); "" for lines that need none. */
  romanize: (lines: string[]) => invoke<string[]>("romanize_lyrics", { lines }),
  /** `later`: songs playing now, shown changed already; their files are written when they stop. */
  writeTags: (ids: string[], changes: Record<string, string>) =>
    invoke<{ saved: number; later: number; change: string | null }>("write_tags", { ids, changes }),
  playlists: () => invoke<Playlist[]>("get_playlists"),
  savePlaylist: (playlist: Playlist) => invoke<Playlist>("save_playlist", { playlist }),
  deletePlaylist: (id: string) => invoke<void>("delete_playlist", { id }),
  setCloseToTray: (enabled: boolean) => invoke<void>("set_close_to_tray", { enabled }),
  setGlobalHotkeys: (enabled: boolean) => invoke<void>("set_global_hotkeys", { enabled }),
  healthReport: () => invoke<HealthReport>("health_report"),
  healthDuplicates: () => invoke<DupGroup[]>("health_duplicates"),
  healthStart: (kind: HealthKind) => invoke<boolean>("health_start", { kind }),
  healthStop: () => invoke<void>("health_stop"),
  healthRemove: (ids: string[]) => invoke<Removed>("health_remove", { ids }),
  sonicStatus: () => invoke<SonicStatus>("sonic_status"),
  similarTracks: (id: string, limit?: number) => invoke<string[]>("similar_tracks", { id, limit }),
  soundRadio: (id: string, count?: number) => invoke<string[]>("sound_radio", { id, count }),
  soundMixes: () => invoke<Mix[]>("sound_mixes"),
  aiSettings: () => invoke<{ settings: AiSettings; hasKey: boolean }>("get_ai_settings"),
  setAiSettings: (settings: AiSettings) => invoke<void>("set_ai_settings", { settings }),
  setAiKey: (text: string) => invoke<{ settings: AiSettings; hasKey: boolean }>("set_ai_key", { text }),
  searchLyrics: (query: string) => invoke<LyricsHit[]>("search_lyrics", { query }),
  useLyrics: (trackId: string, hitId: number) => invoke<Lyrics>("use_lyrics", { trackId, hitId }),
  pasteLyrics: (trackId: string, text: string) => invoke<Lyrics>("paste_lyrics", { trackId, text }),
  hideLyrics: (trackId: string) => invoke<void>("hide_lyrics", { trackId }),
  aiDiscover: (refresh: boolean) => invoke<NewSong[]>("ai_discover", { refresh }),
  listeningProfile: () => invoke<ListeningProfile>("listening_profile"),
  listeningStats: (range: StatsRange) => invoke<ListeningStats>("listening_stats", { range }),
  aiRecommended: () => invoke<Recommendations>("ai_recommended"),
  /** The suggestions the library still doesn't have (asked again when the library changes). */
  aiStillNew: (picks: NewSong[]) => invoke<NewSong[]>("ai_still_new", { picks }),
  aiRecommend: (basis: DiscoverBasis, value: string) => invoke<Recommendations>("ai_recommend", { basis, value }),
  /** Today's picks for this month's listening: made once a day and kept. */
  aiDailyPicks: (refresh: boolean) => invoke<Recommendations>("ai_daily_picks", { refresh }),
  aiFindLyrics: (trackId: string) => invoke<Lyrics>("ai_find_lyrics", { trackId }),
  findSingers: (trackId: string) => invoke<Lyrics>("find_singers", { trackId }),
  aiTest: () => invoke<string>("ai_test"),
  aiPreview: (request: string) => invoke<string>("ai_preview", { request }),
  aiAsk: (request: string) => invoke<AiMix>("ai_ask", { request }),
  aiMixes: (refresh: boolean) => invoke<AiMix[]>("ai_mixes", { refresh }),
  toggleMini: () => invoke<void>("toggle_mini"),
  showMain: () => invoke<void>("show_main"),
};

/** Artwork files replaced in place (a new cover): their addresses get a version, so the old image isn't reused. */
const artVersions: Record<string, number> = (() => {
  try {
    return JSON.parse(localStorage.getItem("cadence-art-versions") ?? "{}");
  } catch {
    return {};
  }
})();

export function artChanged(...paths: string[]) {
  const v = Date.now();
  for (const p of paths) artVersions[p] = v;
  localStorage.setItem("cadence-art-versions", JSON.stringify(artVersions));
}

export const artUrl = (path: string | null | undefined) =>
  path ? convertFileSrc(path) + (artVersions[path] ? `?v=${artVersions[path]}` : "") : "";

export function formatTime(secs: number): string {
  if (!isFinite(secs) || secs < 0) secs = 0;
  const s = Math.floor(secs % 60);
  const m = Math.floor(secs / 60) % 60;
  const h = Math.floor(secs / 3600);
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}` : `${m}:${String(s).padStart(2, "0")}`;
}

export function formatDuration(secs: number): string {
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins} min`;
  return `${Math.floor(mins / 60)} hr ${mins % 60} min`;
}

/** Decibels with a typographic minus: "+3.0 dB", "−8.1 dB". */
export const db = (v: number, digits = 1) => `${v > 0 ? "+" : v < 0 ? "−" : ""}${Math.abs(v).toFixed(digits)} dB`;

/** Splits a credit like "A, B & C feat. D" into its names (no judgement on whether that's right). */
export function splitCredit(credit: string): string[] {
  return credit
    .split(/\s*(?:,|;|\/|&|\bfeat\.?(?=\s)|\bft\.?(?=\s)|\bfeaturing\b|\s[x×]\s)\s*/i)
    .map((s) => s.trim())
    .filter(Boolean);
}

/**
 * Maps each album-artist credit to the artists it should be listed under. A credit is only split when
 * its parts are real artists in the library (credited alone somewhere, or in two or more credits), so
 * "Wuthering Waves & Crywolf" lists under Wuthering Waves while "Simon & Garfunkel" stays one artist.
 */
export function artistIndex(credits: string[]): Map<string, string[]> {
  const unique = [...new Set(credits)];
  const standalone = new Set(unique.filter((c) => splitCredit(c).length === 1).map((c) => c.toLowerCase()));
  const uses = new Map<string, number>();
  for (const c of unique) for (const p of new Set(splitCredit(c).map((x) => x.toLowerCase()))) uses.set(p, (uses.get(p) ?? 0) + 1);
  const map = new Map<string, string[]>();
  for (const c of unique) {
    const parts = splitCredit(c);
    const known = parts.length > 1 ? parts.filter((p) => standalone.has(p.toLowerCase()) || (uses.get(p.toLowerCase()) ?? 0) >= 2) : [];
    map.set(c, known.length ? known : [c]);
  }
  return map;
}

export const khz = (rate: number) => (rate % 1000 === 0 ? `${rate / 1000} kHz` : `${(rate / 1000).toFixed(1)} kHz`);

/** Apple Music's naming: lossless above 48 kHz is "Hi-Res Lossless". */
export function qualityOf(codec: string, sampleRate: number | null): string {
  const lossless = ["FLAC", "ALAC", "WAV", "AIFF", "PCM"].includes(codec.toUpperCase());
  if (!lossless) return codec;
  return (sampleRate ?? 0) > 48000 ? "Hi-Res Lossless" : "Lossless";
}

/** A catalogue match offered in Fix Song Info. */
export interface SongInfo {
  title: string;
  artist: string;
  album: string;
  albumArtist: string;
  year: number | null;
  trackNo: number | null;
  discNo: number | null;
  genre: string;
  coverUrl: string | null;
  thumb: string | null;
  duration: number;
  /** Same title (and artist) and about the same length; or, for a title in another script, the same artists and length. */
  likely: boolean;
  /** The catalogue it's from: "iTunes" or "Deezer". */
  source: string;
  /** Its album in that catalogue, for the album's other songs and genre. */
  albumRef: string | null;
  /** Its iTunes id (the same in every country's store). */
  itunesId?: number | null;
  /** Its names in Latin letters when they're in Japanese, Chinese or Korean ones: Apple's (the US
   *  store's, e.g. "Antanante." by "Riria.") or romanised by Cadence. */
  latin?: { title: string; artist: string; album: string; albumArtist: string; albumRef: string | null; from: string } | null;
}

/** Info Fix all found for a song without tags, to check before saving. */
export interface FoundInfo {
  trackId: string;
  /** What the song is called now (often its file name). */
  now: string;
  title: string;
  artist: string;
  album: string;
  year: number | null;
  cover: string | null;
}

/** A change Cadence made to songs' files, which can be undone. */
export interface ChangeInfo {
  id: string;
  /** Unix seconds. */
  at: number;
  label: string;
  songs: number;
}

/** An album in the iTunes or Deezer catalogue (Fix Album Info). */
export interface AlbumMatch {
  title: string;
  artist: string;
  year: number | null;
  genre: string;
  coverUrl: string | null;
  thumb: string | null;
  tracks: number;
  albumRef: string;
  source: string;
  /** Every song of the album in the library matches one of its songs by length. */
  likely: boolean;
  /** How many of the album's files match one of its songs (for the likeliest few results). */
  matched: number;
}

/** A song of a catalogue album. */
export interface CatalogSong {
  title: string;
  artist: string;
  duration: number;
  trackNo: number | null;
  discNo: number | null;
  deezerId: number | null;
}

/** Which catalogue song (index into the album's songs) a file is; null: not on the album. */
export interface SongAssign {
  trackId: string;
  song: number | null;
}

export interface AlbumPlan {
  songs: CatalogSong[];
  assign: SongAssign[];
  year: number | null;
  genre: string;
}

/** A background job's progress (Fix all: `found` songs tagged so far). */
export interface JobProgress {
  done: number;
  total: number;
  found?: number;
  running: boolean;
}

/** An artist in the library under two names. */
export interface ArtistPair {
  native: string;
  nativeSongs: number;
  latin: string;
  latinSongs: number;
}

export interface ArtistPairs {
  pairs: ArtistPair[];
  /** Names in Japanese or Chinese letters not checked yet. */
  unchecked: number;
  running: boolean;
}

/** What Fix Song Info saved. */
export interface SongInfoSaved {
  /** The song is playing: its file is written when it finishes. */
  later: boolean;
  /** The change, to undo it. */
  change: string | null;
  /** Other songs of its album given the album's info. */
  others: number;
  /** Of those, the ones found on the catalogue's album (own title, artist and track number). */
  matched: number;
}

/** A picture the user chose for an artist (large and small files). */
export interface ArtistImage {
  large: string;
  small: string;
}

/** An artist photo from the online catalogue. */
export interface ArtistPhoto {
  name: string;
  picture: string;
  thumb: string;
  fans: number;
}

/** A cover offered in Change Cover. */
export interface CoverResult {
  title: string;
  artist: string;
  year: number | null;
  url: string;
  thumb: string;
  source: string;
  /** Same album title and artist. */
  likely: boolean;
}

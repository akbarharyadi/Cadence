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
  year: number | null;
  trackNo: number | null;
  discNo: number | null;
  duration: number;
  sampleRate: number | null;
  bitDepth: number | null;
  channels: number | null;
  bitrate: number | null;
  codec: string;
  added: number;
  gain: SongGain;
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
  source: SourceInfo | null;
  output: OutputInfo;
  error: string | null;
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
  /** Word timings ("enhanced" LRC), when the lyrics have them. */
  words?: { time: number; text: string }[];
}

export interface Lyrics {
  source: "embedded" | "file" | "lrclib" | "pasted" | "ai" | "";
  synced: LyricLine[];
  plain: string;
  instrumental: boolean;
  /** Why nothing was found online: "off", "missing", "unavailable", or "" when found. */
  lookup: "off" | "missing" | "unavailable" | "hidden" | "";
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

export interface AiSettings {
  provider: AiProvider;
  cloudUrl: string;
  cloudModel: string;
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

/** A song suggested by the DJ that isn't in the library. */
export interface NewSong {
  artist: string;
  title: string;
  why: string;
  album?: string;
  /** Album cover, or the artist's picture when `imageIsArtist`; empty if unknown. */
  image?: string;
  imageIsArtist?: boolean;
}

/** Stores that sell downloads (lossless where they can), searched for a song. */
export const STORES: { name: string; url: (q: string) => string }[] = [
  { name: "Qobuz", url: (q) => `https://www.qobuz.com/us-en/search?q=${q}` },
  { name: "Bandcamp", url: (q) => `https://bandcamp.com/search?q=${q}&item_type=t` },
  { name: "OTOTOY", url: (q) => `https://ototoy.jp/find/?q=${q}` },
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
  play: (ids: string[], start = 0) => invoke<void>("play_tracks", { ids, start }),
  enqueue: (ids: string[], next: boolean) => invoke<void>("enqueue", { ids, next }),
  player: (action: "play" | "pause" | "toggle" | "next" | "prev") => invoke<void>("player", { action }),
  seek: (position: number) => invoke<void>("seek", { position }),
  jump: (index: number) => invoke<void>("jump", { index }),
  removeFromQueue: (index: number) => invoke<void>("remove_from_queue", { index }),
  setVolume: (volume: number) => invoke<void>("set_volume", { volume }),
  setRepeat: (repeat: Repeat) => invoke<void>("set_repeat", { repeat }),
  status: () => invoke<Status>("get_status"),
  dsp: () => invoke<DspSettings>("get_dsp"),
  setDsp: (dsp: DspSettings) => invoke<void>("set_dsp", { dsp }),
  eqPresets: () => invoke<Preset[]>("eq_presets"),
  parseEqFile: (args: { text?: string; path?: string }) => invoke<{ preamp: number; filters: Filter[] }>("parse_eq_file", args),
  reanalyze: () => invoke<void>("reanalyze"),
  history: () => invoke<HistoryStats>("get_history"),
  appSettings: () => invoke<{ closeToTray: boolean; lyricsOnline: boolean; listenbrainzUser: string }>("get_app_settings"),
  connectListenBrainz: (token: string) => invoke<string>("connect_listenbrainz", { token }),
  disconnectListenBrainz: () => invoke<void>("disconnect_listenbrainz"),
  setLyricsOnline: (enabled: boolean) => invoke<void>("set_lyrics_online", { enabled }),
  lyrics: (trackId: string) => invoke<Lyrics>("get_lyrics", { trackId }),
  writeTags: (ids: string[], changes: Record<string, string>) => invoke<number>("write_tags", { ids, changes }),
  playlists: () => invoke<Playlist[]>("get_playlists"),
  savePlaylist: (playlist: Playlist) => invoke<Playlist>("save_playlist", { playlist }),
  deletePlaylist: (id: string) => invoke<void>("delete_playlist", { id }),
  setCloseToTray: (enabled: boolean) => invoke<void>("set_close_to_tray", { enabled }),
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
  aiFindLyrics: (trackId: string) => invoke<Lyrics>("ai_find_lyrics", { trackId }),
  aiTest: () => invoke<string>("ai_test"),
  aiPreview: (request: string) => invoke<string>("ai_preview", { request }),
  aiAsk: (request: string) => invoke<AiMix>("ai_ask", { request }),
  aiMixes: (refresh: boolean) => invoke<AiMix[]>("ai_mixes", { refresh }),
  toggleMini: () => invoke<void>("toggle_mini"),
  showMain: () => invoke<void>("show_main"),
};

export const artUrl = (path: string | null | undefined) => (path ? convertFileSrc(path) : "");

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

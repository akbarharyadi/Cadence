// Fixtures and a fake backend, so a test reads as "given this library, when the user does that".
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Album, Mix, Playlist, Status, Track } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";

let seq = 0;

/** A FLAC song, as the backend's `get_library` would list it. */
export function makeTrack(over: Partial<Track> = {}): Track {
  const n = ++seq;
  return {
    id: `t${n}`,
    path: `D:\\Music\\Artist\\Album\\${String(n).padStart(2, "0")} Song ${n}.flac`,
    title: `Song ${n}`,
    artist: "Artist",
    album: "Album",
    albumArtist: "Artist",
    albumId: "al1",
    genre: "Rock",
    year: 2020,
    trackNo: n,
    discNo: 1,
    duration: 200,
    sampleRate: 44100,
    bitDepth: 16,
    channels: 2,
    bitrate: 900,
    codec: "FLAC",
    size: 30_000_000,
    added: 1_700_000_000,
    gain: { track: null, album: null },
    ...over,
  };
}

export function makeAlbum(tracks: Track[], over: Partial<Album> = {}): Album {
  const first = tracks[0];
  return {
    id: first?.albumId ?? "al1",
    title: first?.album ?? "Album",
    artist: first?.albumArtist ?? "Artist",
    year: first?.year ?? 2020,
    genre: first?.genre ?? "Rock",
    art: null,
    artSmall: null,
    trackIds: tracks.map((t) => t.id),
    duration: tracks.reduce((s, t) => s + t.duration, 0),
    quality: "Lossless",
    format: "FLAC",
    added: 1_700_000_000,
    accent: null,
    tint: null,
    ...over,
  };
}

export function makePlaylist(trackIds: string[], over: Partial<Playlist> = {}): Playlist {
  return { id: "pl1", name: "Road trip", trackIds, smart: null, created: 1, updated: 1, ...over };
}

export function makeStatus(over: Partial<Status> = {}): Status {
  return {
    queue: [],
    index: null,
    playing: false,
    position: 0,
    duration: 0,
    volume: 1,
    repeat: "off",
    shuffle: false,
    source: { codec: "FLAC", sampleRate: 44100, bits: 16, channels: 2 },
    output: {
      device: "Speakers",
      sampleRate: 44100,
      channels: 2,
      mode: "shared",
      resampling: false,
      eq: false,
      eqHeadroom: 0,
      crossfeed: false,
      leveling: null,
      tone: null,
      remixing: false,
      unaltered: true,
      bitPerfect: false,
      bits: 16,
      deviceVolumeDb: null,
      note: null,
      peakDb: -6,
      clipping: false,
    },
    error: null,
    stopAfter: null,
    ...over,
  };
}

export function makeMix(name: string, trackIds: string[] = []): Mix {
  return { name, description: `${name}, by sound`, trackIds };
}

/** A small library: `count` songs by one artist on one album (plus any `extra` songs). */
export function makeLibrary(count = 5, extra: Track[] = []) {
  const tracks = Array.from({ length: count }, (_, i) => makeTrack({ title: `Song ${String.fromCharCode(65 + i)}`, trackNo: i + 1 }));
  const all = [...tracks, ...extra];
  return { tracks: all, albums: [makeAlbum(tracks)] };
}

export type Handler = (args: Record<string, unknown>) => unknown;
export interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

/**
 * Installs Tauri's mockIPC (events included, so `listen` works) and records every command the page
 * invokes. `handlers` answer commands by name; anything else gets a harmless empty answer.
 */
export function mockBackend(handlers: Record<string, Handler> = {}) {
  const calls: Call[] = [];
  mockIPC(
    (cmd, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      calls.push({ cmd, args });
      const h = handlers[cmd];
      if (h) return h(args);
      // `save_playlist` and friends echo what they were given: the page then shows what was saved.
      if (cmd === "save_playlist") return args.playlist;
      if (cmd === "plugin:app|version") return "0.0.0-test";
      return null;
    },
    { shouldMockEvents: true },
  );
  return {
    calls,
    /** Every call to one command, in order. */
    to: (cmd: string) => calls.filter((c) => c.cmd === cmd),
    /** The arguments of the last call to a command. */
    last: (cmd: string) => calls.filter((c) => c.cmd === cmd).at(-1)?.args,
  };
}

/** Puts the `app` singleton back to a clean slate and gives it a library. */
export function resetApp(init: { tracks?: Track[]; albums?: Album[]; playlists?: Playlist[]; status?: Status | null } = {}) {
  const priv = app as unknown as { backMeta: unknown[]; forwardMeta: unknown[]; lastCurrent: unknown; lastAlbum: unknown };
  app.tracks = init.tracks ?? [];
  app.albums = init.albums ?? [];
  app.playlists = init.playlists ?? [];
  app.status = init.status ?? null;
  app.view = { name: "home" };
  app.section = "home";
  app.history = [];
  app.forwardHistory = [];
  priv.backMeta = [];
  priv.forwardMeta = [];
  priv.lastCurrent = null;
  priv.lastAlbum = null;
  app.search = "";
  app.menu = null;
  app.toast = null;
  app.toastAction = null;
  app.picker = null;
  app.queueOpen = false;
  app.nowPlayingOpen = false;
  app.marks = {};
  app.listening = null;
  app.mixes = [];
  app.aiMixes = [];
  app.aiNew = [];
  app.ai = null;
  app.daily = null;
  app.discover = null;
  app.profile = null;
  app.duplicates = [];
  app.health = null;
  app.attentionPrefs = { ignored: [], hidden: [] };
  app.artistPairsCount = 0;
  app.identity = null;
  app.searchKeys = null;
  app.namesJob = null;
  app.position = 0;
  app.dailyState = "idle";
  app.discoverState = "idle";
  app.aiNewState = "idle";
  app.aiMixesState = "idle";
}

/** Lets pending promises (mocked IPC answers) and Svelte's effects settle. */
export const settle = () => new Promise<void>((r) => setTimeout(r, 0));

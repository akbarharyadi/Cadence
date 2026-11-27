/** Sync with phone: the shapes the backend's phone commands return (src-tauri/src/phone). */

export type PhoneKind = "playlist" | "album" | "artist";

/** What's kept on the phone, for the menus. */
export interface PhoneKept {
  playlists: string[];
  albums: string[];
  /** Every library spelling of each kept artist. */
  artistNames: string[];
  top: number;
}

export interface PhoneChoice {
  playlists: string[];
  albums: string[];
  artists: string[];
  top: number;
  phones: PairedPhone[];
}

export interface PairedPhone {
  id: string;
  name: string;
  /** Unix seconds. */
  paired: number;
  /** Unix seconds of its last sync (0: never). */
  lastSeen: number;
}

/** What the choices come to. */
export interface PhoneSummary {
  choice: PhoneChoice;
  songs: number;
  bytes: number;
  /** Songs still being prepared (sent on a later sync). */
  pending: number;
  /** Songs of single-file albums, not sent yet. */
  leftOut: number;
  /** Kept playlists that are smart or gone, not sent. */
  notSentPlaylists: number;
}

/** The computer's side: on, its address, the pairing code while pairing is open. */
export interface PhoneService {
  enabled: boolean;
  on: boolean;
  address: string;
  computer: string;
  code: string | null;
  codeUntil: number;
  phones: PairedPhone[];
  error: string;
}

/** "Preparing songs for the phone" ("phone-keys-progress" events). */
export interface PhoneKeysJob {
  running: boolean;
  paused: boolean;
  done: number;
  total: number;
}

/** The phone's side: the computer it's paired with and its last sync. */
export interface PhoneLink {
  /** This device syncs from a computer (the phone). */
  supported: boolean;
  paired: boolean;
  computer: string;
  address: string;
  /** Where synced music goes on the phone. */
  folder: string;
  /** Songs this phone copied from the computer. */
  songs: number;
  /** Unix seconds (0: never). */
  lastSync: number;
  lastResult: string;
}

/** A sync on the phone ("phone-sync-progress" events). */
export interface PhoneSyncJob {
  running: boolean;
  paused: boolean;
  phase: "" | "connecting" | "copying" | "removing";
  done: number;
  total: number;
  bytesDone: number;
  bytesTotal: number;
  current: string;
  /** How the last sync ended, in words. */
  message: string;
  failed: boolean;
}

/** The "most played" counts offered. */
export const TOP_CHOICES = [0, 100, 200, 500] as const;

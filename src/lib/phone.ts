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

/** The "most played" counts offered. */
export const TOP_CHOICES = [0, 100, 200, 500] as const;

// Playlists in and out of files: what the Export… and Import playlist… dialogs are showing, and the
// steps that start them (PlaylistTransfer.svelte draws the dialogs; they're mounted once, in the page).
import { open } from "@tauri-apps/plugin-dialog";
import { api, type Playlist, type PlaylistFiles, type PlaylistImport } from "#lib/api.js";
import { evaluate } from "#lib/smart.js";
import { app } from "#lib/store.svelte.js";

export const onAndroid = typeof navigator !== "undefined" && navigator.userAgent.includes("Android");
const onWindows = typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");

/** What is being exported: one playlist's songs, or every playlist (one file each). */
export type ExportTarget = { kind: "one"; name: string; ids: string[] } | { kind: "all"; items: { name: string; ids: string[] }[] };

class Transfer {
  exporting = $state<ExportTarget | null>(null);
  /** Playlist files being read and matched. */
  reading = $state(false);
  /** The files read, one after the other: the first is the one the dialog asks about. */
  imports = $state<PlaylistImport[]>([]);
  /** Android: the playlist files in Documents › Cadence, to choose from. */
  choosing = $state<PlaylistFiles | null>(null);
}
export const transfer = new Transfer();

/** The songs a playlist has now: a smart playlist's are worked out from its rules. */
export function playlistIds(p: Playlist): string[] {
  if (p.smart) return evaluate(p.smart, app.tracks, app.listening, app.marks, p.id).map((t) => t.id);
  return [...p.trackIds];
}

export function startExport(p: Playlist) {
  transfer.exporting = { kind: "one", name: p.name, ids: playlistIds(p) };
}

export function startExportAll() {
  transfer.exporting = { kind: "all", items: app.playlists.map((p) => ({ name: p.name, ids: playlistIds(p) })) };
}

/** Reads and matches the files; the first result is then asked about. */
export async function importPaths(paths: string[]) {
  if (!paths.length) return;
  transfer.reading = true;
  try {
    transfer.imports = await api.playlistImport(paths);
  } catch (e) {
    app.showToast(`Couldn’t read the playlist: ${e}`);
  } finally {
    transfer.reading = false;
  }
}

/** "Import playlist…": the open dialog (several files at once); on Android, the files in Documents › Cadence. */
export async function startImport() {
  try {
    if (onAndroid) {
      // The system's file picker hands back an address Cadence can't read.
      transfer.choosing = await api.playlistImportFiles();
      return;
    }
    const picked = await open({
      title: "Import playlists",
      multiple: true,
      filters: [{ name: "Playlists", extensions: ["m3u", "m3u8", "csv", "xspf"] }],
    });
    const paths = Array.isArray(picked) ? picked : typeof picked === "string" && picked ? [picked] : [];
    await importPaths(paths);
  } catch (e) {
    app.showToast(`Couldn’t import a playlist: ${e}`);
  }
}

/** The window's saved choices for the export dialog. */
const OPTIONS = "cadence-playlist-export";
export function savedExportOptions(): { format: "m3u8" | "csv"; relative: boolean; forwardSlashes: boolean; crlf: boolean } {
  const base = { format: "m3u8" as const, relative: false, forwardSlashes: !onWindows, crlf: false };
  try {
    const o = JSON.parse(localStorage.getItem(OPTIONS) ?? "null");
    if (o && typeof o === "object") {
      return {
        format: o.format === "csv" ? "csv" : "m3u8",
        relative: o.relative === true,
        forwardSlashes: typeof o.forwardSlashes === "boolean" ? o.forwardSlashes : base.forwardSlashes,
        crlf: o.crlf === true,
      };
    }
  } catch {
    // Storage unavailable or damaged: the defaults.
  }
  return base;
}

export function keepExportOptions(o: ReturnType<typeof savedExportOptions>) {
  try {
    localStorage.setItem(OPTIONS, JSON.stringify(o));
  } catch {
    // Not kept: the next export starts from the defaults.
  }
}

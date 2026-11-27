// Sync with phone in the window. On the computer: what's kept on the phone (for the "Keep on Phone"
// menu items) and the job preparing songs for it. On the phone: the sync from the computer, and the
// Android service that keeps it going with the screen off. Shapes in phone.ts; the design in
// docs/research-phone-sync.md.

import { listen } from "@tauri-apps/api/event";
import { api } from "#lib/api.js";
import type { PhoneKeysJob, PhoneKept, PhoneKind, PhoneSyncJob } from "#lib/phone.js";
import type { AndroidBridge, MenuItem } from "#lib/store.svelte.js";

export const onPhone = typeof navigator !== "undefined" && /Android|iPhone|iPad/.test(navigator.userAgent);

/** Whether `kept` has this playlist, album, or (under any of its library names) artist. */
export function isKept(kept: PhoneKept | null, kind: PhoneKind, id: string): boolean {
  if (!kept || !id) return false;
  if (kind === "playlist") return kept.playlists.includes(id);
  if (kind === "album") return kept.albums.includes(id);
  const name = id.toLowerCase();
  return kept.artistNames.some((n) => n.toLowerCase() === name);
}

/** The phone's model from the browser's description ("Pixel 8"), for the computer's list. */
export function phoneName(ua = typeof navigator !== "undefined" ? navigator.userAgent : ""): string {
  const model = /Android [\d.]+; ([^;)]+)\)/.exec(ua)?.[1]?.trim();
  return model && model !== "K" ? model : "Android phone";
}

/** The notification's line for a sync in progress. */
export function syncLine(j: PhoneSyncJob): string {
  if (j.phase === "connecting") return "Connecting to your computer";
  if (j.phase === "removing") return "Removing songs no longer chosen";
  return j.current ? `${j.done + 1} of ${j.total}: ${j.current}` : `${j.done} of ${j.total}`;
}

const bridge = (): AndroidBridge | undefined => (globalThis as { CadenceAndroid?: AndroidBridge }).CadenceAndroid;

class PhoneSync {
  kept = $state<PhoneKept | null>(null);
  /** The computer: the job preparing songs for the phone, while it runs. */
  keysJob = $state<PhoneKeysJob | null>(null);
  /** The phone: the sync from the computer, while it runs. */
  syncJob = $state<PhoneSyncJob | null>(null);
  /** The phone: how the last sync ended. */
  lastSync = $state<PhoneSyncJob | null>(null);
  #started = false;
  /** What the notification last showed, so it's only told of changes. */
  #shown = "";

  /** Follows what this side needs (once). */
  start() {
    if (this.#started) return;
    this.#started = true;
    if (onPhone) {
      void listen<PhoneSyncJob>("phone-sync-progress", (e) => this.onSync(e.payload));
      return;
    }
    void this.refresh();
    void listen("phone-changed", () => void this.refresh());
    void listen<PhoneKeysJob>("phone-keys-progress", (e) => (this.keysJob = e.payload?.running ? e.payload : null));
  }

  onSync(j: PhoneSyncJob | null) {
    if (!j) return;
    this.syncJob = j.running ? j : null;
    if (!j.running) this.lastSync = j;
    const line = j.running ? syncLine(j) : "";
    const shown = `${j.running}|${line}|${j.done}|${j.total}`;
    if (shown === this.#shown) return;
    this.#shown = shown;
    bridge()?.syncProgress?.(j.running, line, j.done, j.total);
  }

  async refresh() {
    try {
      this.kept = await api.phoneKept();
    } catch {
      // Keeps what it had: the menus then show the last known state.
    }
  }

  isKept(kind: PhoneKind, id: string) {
    return isKept(this.kept, kind, id);
  }

  /** "Keep on Phone" or "Remove from Phone" for a menu; nothing on the phone itself. */
  menuItem(kind: PhoneKind, id: string): MenuItem[] {
    if (onPhone || !id) return [];
    const on = this.isKept(kind, id);
    return [{ label: on ? "Remove from Phone" : "Keep on Phone", icon: "phone", action: () => void this.keep(kind, id, !on) }];
  }

  async keep(kind: PhoneKind, id: string, on: boolean) {
    await api.phoneKeep(kind, id, on);
    await this.refresh();
  }
}

export const phoneSync = new PhoneSync();

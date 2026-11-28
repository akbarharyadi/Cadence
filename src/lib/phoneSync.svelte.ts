// Sync with phone in the window. On the computer: what's kept on the phone (for the "Keep on Phone"
// menu items) and the job preparing songs for it. On the phone: the sync from the computer, and the
// Android service that keeps it going with the screen off. Shapes in phone.ts; the design in
// docs/research-phone-sync.md.

import { listen } from "@tauri-apps/api/event";
import { api } from "#lib/api.js";
import type { PhoneKeysJob, PhoneKept, PhoneKind, PhoneOffer, PhoneSyncJob } from "#lib/phone.js";
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

/** What the window does after a pairing link: say why it wasn't taken, or that it paired. */
/** The window's part of pairing: toasts and the Settings page (the app). */
interface PairingHost {
  showToast: (msg: string) => void;
  go: (view: { name: "settings" }) => void;
}
let host: PairingHost | undefined;

const bridge = (): AndroidBridge | undefined => (globalThis as { CadenceAndroid?: AndroidBridge }).CadenceAndroid;

class PhoneSync {
  kept = $state<PhoneKept | null>(null);
  /** The computer: the job preparing songs for the phone, while it runs. */
  keysJob = $state<PhoneKeysJob | null>(null);
  /** The phone: the sync from the computer, while it runs. */
  syncJob = $state<PhoneSyncJob | null>(null);
  /** The phone: how the last sync ended. */
  lastSync = $state<PhoneSyncJob | null>(null);
  /** The phone: a computer's pairing QR code the camera opened, waiting for a yes. */
  offer = $state<PhoneOffer | null>(null);
  pairing = $state(false);
  pairError = $state("");
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

  /** The phone: follows pairing links the camera opens (also the one Cadence was started with). */
  watchPairing(app: PairingHost) {
    host = app;
    (window as unknown as { __cadencePair: () => void }).__cadencePair = () => void this.takePairLink();
    void this.takePairLink();
  }

  /** The pairing link the camera opened (if any): checked, then shown as a question. */
  async takePairLink(link = bridge()?.takePairLink?.() ?? "") {
    if (!link) return;
    this.pairError = "";
    try {
      this.offer = await api.phonePairOffer(link);
    } catch (e) {
      this.offer = null;
      host?.showToast(String(e));
    }
  }

  decline() {
    this.offer = null;
    this.pairError = "";
  }

  /** Yes: pairs with the offered computer; the Settings page then shows the sync. */
  async accept() {
    const o = this.offer;
    if (!o || this.pairing) return;
    this.pairing = true;
    this.pairError = "";
    try {
      const link = await api.phoneLinkPair(o.address, o.code, phoneName());
      this.offer = null;
      host?.showToast(`Paired with ${link.computer || o.computer || "your computer"}. Tap Sync now.`);
      host?.go({ name: "settings" });
    } catch (e) {
      this.pairError = String(e);
    } finally {
      this.pairing = false;
    }
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

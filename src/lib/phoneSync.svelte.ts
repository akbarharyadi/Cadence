// Sync with phone, the computer's side in the window: what's kept on the phone (for the
// "Keep on Phone" menu items) and the job preparing songs for it. Shapes in phone.ts; the design in
// docs/research-phone-sync.md. On the phone itself none of this shows: it's the one that pulls.

import { listen } from "@tauri-apps/api/event";
import { api } from "#lib/api.js";
import type { PhoneKeysJob, PhoneKept, PhoneKind } from "#lib/phone.js";
import type { MenuItem } from "#lib/store.svelte.js";

export const onPhone = typeof navigator !== "undefined" && /Android|iPhone|iPad/.test(navigator.userAgent);

/** Whether `kept` has this playlist, album, or (under any of its library names) artist. */
export function isKept(kept: PhoneKept | null, kind: PhoneKind, id: string): boolean {
  if (!kept || !id) return false;
  if (kind === "playlist") return kept.playlists.includes(id);
  if (kind === "album") return kept.albums.includes(id);
  const name = id.toLowerCase();
  return kept.artistNames.some((n) => n.toLowerCase() === name);
}

class PhoneSync {
  kept = $state<PhoneKept | null>(null);
  /** The job preparing songs for the phone, while it runs. */
  keysJob = $state<PhoneKeysJob | null>(null);
  #started = false;

  /** Loads what's kept once and follows changes (desktop only). */
  start() {
    if (this.#started || onPhone) return;
    this.#started = true;
    void this.refresh();
    void listen("phone-changed", () => void this.refresh());
    void listen<PhoneKeysJob>("phone-keys-progress", (e) => (this.keysJob = e.payload?.running ? e.payload : null));
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

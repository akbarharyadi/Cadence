import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import BackupSettings from "#lib/components/BackupSettings.svelte";
import type { BackupInfo, BackupPreview, BackupRestored } from "#lib/api.js";
import { announceInterrupted } from "#lib/backupNotice.js";
import Settings from "#lib/views/Settings.svelte";
import { app } from "#lib/store.svelte.js";
import { mockBackend, resetApp, settle } from "./helpers";

const snapshot = "C:\\Users\\me\\AppData\\Roaming\\cadence\\backups\\auto-1794700000.zip";
const info: BackupInfo = {
  lastAuto: 1_794_700_000,
  snapshots: [{ path: snapshot, name: "auto-1794700000.zip", kind: "auto", created: 1_794_700_000, size: 120_000 }],
  folder: "",
  folderError: "",
  exportDir: "",
  dir: "C:\\Users\\me\\AppData\\Roaming\\cadence\\backups",
};
const preview: BackupPreview = {
  path: snapshot,
  app: "0.2.40",
  created: 1_794_700_000,
  kind: "auto",
  replaces: ["Playlists (4)", "Favorites and ratings (12 songs)", "Listening history (5,000 plays)"],
};
const restored: BackupRestored = {
  files: 9,
  relinked: 3,
  unresolved: 2,
  unresolvedHistory: 0,
  safety: "C:\\…\\backups\\safety-1794800000.zip",
  lyricOffsets: '{"t1":0.5}',
  app: "0.2.40",
  created: 1_794_700_000,
};

function open() {
  const backend = mockBackend({
    backup_info: () => info,
    backup_preview: () => preview,
    backup_restore: () => restored,
    get_skips: () => ({ ids: [], instrumentals: false }),
    get_folders: () => [],
    artist_images: () => ({}),
  });
  resetApp();
  render(BackupSettings);
  return { backend, user: userEvent.setup() };
}

const interrupted = {
  at: 1_794_900_000,
  backup: snapshot,
  rolledBack: true,
  error: "",
  message: "A restore was interrupted — your data was put back as it was before.",
};

async function restoreWith(result: BackupRestored) {
  const backend = mockBackend({
    backup_info: () => info,
    backup_preview: () => preview,
    backup_restore: () => result,
    get_skips: () => ({ ids: [], instrumentals: false }),
    get_folders: () => [],
    artist_images: () => ({}),
  });
  resetApp();
  render(BackupSettings);
  const user = userEvent.setup();
  await settle();
  await user.click(screen.getByRole("button", { name: /Restore the backup from/ }));
  await settle();
  await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Restore" }));
  await settle();
  await settle();
  return { backend, status: screen.getByText(/Restored the backup from/).closest("[role=status]") as HTMLElement };
}

describe("Settings › Backup: what the restore did not do", () => {
  test("an interrupted restore is announced at the top of the section with its message", async () => {
    mockBackend({ backup_info: () => ({ ...info, interrupted }) });
    resetApp();
    render(BackupSettings);
    await settle();
    expect(screen.getByRole("alert").textContent).toContain(interrupted.message);
  });

  test("with no interruption there is no notice", async () => {
    mockBackend({ backup_info: () => ({ ...info, interrupted: null }) });
    resetApp();
    render(BackupSettings);
    await settle();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("files held back are listed with the advice to restart and restore again", async () => {
    const { status } = await restoreWith({ ...restored, heldBack: ["settings.json", "marks.json"] });
    expect(status.textContent).toMatch(/2 files couldn't be replaced/);
    expect(status.textContent).toContain("settings.json, marks.json");
    expect(status.textContent).toMatch(/start it again and restore this backup once more/);
  });

  test("older history set aside is said, with the folder", async () => {
    const { status } = await restoreWith({ ...restored, historyAside: "C:\\data\\history-aside-1" });
    expect(status.textContent).toMatch(/set aside, not deleted: C:\\data\\history-aside-1/);
  });

  test("a clean restore mentions neither", async () => {
    const { status } = await restoreWith({ ...restored, heldBack: [], historyAside: "" });
    expect(status.textContent).not.toMatch(/couldn't be replaced|set aside/);
  });
});

describe("An interrupted restore is announced once at startup", () => {
  test("the toast carries the message, and is not repeated for the same interruption", () => {
    localStorage.removeItem("cadence-interrupted-seen");
    const shown: string[] = [];
    announceInterrupted({ ...info, interrupted }, (m) => shown.push(m));
    announceInterrupted({ ...info, interrupted }, (m) => shown.push(m));
    expect(shown).toEqual([interrupted.message]);
    announceInterrupted({ ...info, interrupted: { ...interrupted, at: interrupted.at + 1 } }, (m) => shown.push(m));
    expect(shown).toHaveLength(2);
  });

  test("nothing is shown without an interruption", () => {
    const shown: string[] = [];
    announceInterrupted(info, (m) => shown.push(m));
    announceInterrupted(null, (m) => shown.push(m));
    expect(shown).toEqual([]);
  });

  test("app.init shows it as a toast", async () => {
    localStorage.removeItem("cadence-interrupted-seen");
    mockBackend({
      backup_info: () => ({ ...info, interrupted }),
      get_library: () => ({ tracks: [], albums: [] }),
      get_folders: () => [],
      get_history: () => ({ plays: {}, skips: {}, lastPlayed: {}, recentAlbums: [], totalListened: 0 }),
      get_playlists: () => [],
      get_status: () => null,
    });
    resetApp();
    await app.init().catch(() => {});
    await settle();
    expect(app.toast).toBe(interrupted.message);
  });
});

describe("Settings › Backup: restoring asks first", () => {
  test("a snapshot's Restore shows what will be replaced, and Cancel restores nothing", async () => {
    const { backend, user } = open();
    await settle();
    expect(screen.getByText(/Last automatic copy/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Restore the backup from/ }));
    await settle();
    const dialog = screen.getByRole("dialog", { name: "Restore this backup?" });
    expect(backend.last("backup_preview")).toEqual({ path: snapshot });
    for (const line of preview.replaces) expect(within(dialog).getByText(line)).toBeInTheDocument();
    expect(within(dialog).getByText(/copied first/)).toBeInTheDocument();
    // Nothing is restored just by asking.
    expect(backend.to("backup_restore")).toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await settle();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(backend.to("backup_restore")).toHaveLength(0);
  });

  test("confirming restores that backup and says what was put back and re-linked", async () => {
    const { backend, user } = open();
    await settle();
    await user.click(screen.getByRole("button", { name: /Restore the backup from/ }));
    await settle();
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Restore" }));
    await settle();
    await settle();

    expect(backend.to("backup_restore")).toHaveLength(1);
    expect(backend.last("backup_restore")).toEqual({ path: snapshot });
    const status = screen.getByText(/Restored the backup from/).closest("[role=status]") as HTMLElement;
    expect(within(status).getByText(/9 files put back/)).toBeInTheDocument();
    expect(status.textContent).toMatch(/3 songs found again/);
    expect(status.textContent).toMatch(/2 songs of your playlists, favorites or queue aren't in your library now/);
    // The lyrics timing nudges come back into this window's storage.
    expect(localStorage.getItem("cadence-lyric-offsets")).toBe('{"t1":0.5}');
    // And the window reads again what the restore changed (sound settings, settings, AI, lists…).
    for (const cmd of ["get_dsp", "get_ai_settings", "get_playlists", "get_marks", "get_history", "get_status", "get_skips", "get_folders", "artist_images"]) {
      expect(backend.to(cmd).length, cmd).toBeGreaterThanOrEqual(1);
    }
    expect(app.restored).toBeGreaterThan(0);
  });

  test("Settings' own toggles show the restored settings", async () => {
    let lyricsOnline = true;
    mockBackend({
      get_app_settings: () => ({ closeToTray: true, globalHotkeys: true, lyricsOnline, fixTagsOnline: true, listenbrainzUser: "" }),
      backup_info: () => info,
    });
    resetApp();
    render(Settings);
    await settle();
    const toggle = screen.getByRole("switch", { name: "Find lyrics online" });
    expect(toggle).toHaveAttribute("aria-checked", "true");
    lyricsOnline = false;
    app.restored++;
    await settle();
    await settle();
    expect(toggle).toHaveAttribute("aria-checked", "false");
  });

  test("a backup that can't be used says why and asks nothing", async () => {
    const { backend, user } = open();
    backend.calls.length = 0;
    mockBackend({
      backup_info: () => info,
      backup_preview: () => {
        throw "This backup was made by Cadence 9.0.0, newer than this one (0.2.40). Update Cadence first, then restore it.";
      },
    });
    await settle();
    await user.click(screen.getByRole("button", { name: /Restore the backup from/ }));
    await settle();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("alert").textContent).toMatch(/newer than this one/);
  });
});

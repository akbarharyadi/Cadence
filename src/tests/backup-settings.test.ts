import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import BackupSettings from "#lib/components/BackupSettings.svelte";
import type { BackupInfo, BackupPreview, BackupRestored } from "#lib/api.js";
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

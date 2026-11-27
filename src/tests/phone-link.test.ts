import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test, vi } from "vitest";
import PhoneLinkSettings from "#lib/components/settings/PhoneLinkSettings.svelte";
import type { PhoneLink, PhoneSyncJob } from "#lib/phone.js";
import { phoneName, phoneSync, syncLine } from "#lib/phoneSync.svelte.js";
import { mockBackend, settle } from "./helpers";

const unpaired: PhoneLink = { supported: true, paired: false, computer: "", address: "", folder: "/storage/emulated/0/Music/Cadence", songs: 0, lastSync: 0, lastResult: "" };
const paired: PhoneLink = { ...unpaired, paired: true, computer: "DESK", address: "192.168.1.20:47300", songs: 120, lastSync: 1_900_000_000, lastResult: "Up to date." };
const job = (over: Partial<PhoneSyncJob> = {}): PhoneSyncJob => ({
  running: true,
  paused: false,
  phase: "copying",
  done: 2,
  total: 10,
  bytesDone: 50_000_000,
  bytesTotal: 400_000_000,
  current: "IGNITE",
  message: "",
  failed: false,
  ...over,
});

afterEach(() => {
  phoneSync.syncJob = null;
  phoneSync.lastSync = null;
  delete (globalThis as { CadenceAndroid?: unknown }).CadenceAndroid;
});

describe("the phone's side", () => {
  test("the phone's name comes from its model, or a plain fallback", () => {
    expect(phoneName("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36")).toBe("Pixel 8");
    expect(phoneName("Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36")).toBe("Android phone");
    expect(phoneName("")).toBe("Android phone");
  });

  test("the notification line says which song is being copied", () => {
    expect(syncLine(job())).toBe("3 of 10: IGNITE");
    expect(syncLine(job({ phase: "connecting" }))).toBe("Connecting to your computer");
  });

  test("the notification is told only when its line changes, and when the sync ends", () => {
    const syncProgress = vi.fn();
    (globalThis as { CadenceAndroid?: unknown }).CadenceAndroid = { syncProgress };
    phoneSync.onSync(job());
    phoneSync.onSync(job({ bytesDone: 60_000_000 }));
    expect(syncProgress).toHaveBeenCalledTimes(1);
    phoneSync.onSync(job({ running: false, phase: "", message: "Done: copied 10 songs." }));
    expect(syncProgress).toHaveBeenLastCalledWith(false, "", 2, 10);
    expect(phoneSync.syncJob).toBeNull();
    expect(phoneSync.lastSync?.message).toBe("Done: copied 10 songs.");
  });
});

describe("Settings › Sync with computer", () => {
  test("pairs with the address and the 6-digit code", async () => {
    let now = unpaired;
    const backend = mockBackend({ phone_link: () => now, phone_link_pair: () => (now = paired) });
    render(PhoneLinkSettings);
    await settle();
    const pair = screen.getByRole("button", { name: "Pair" }) as HTMLButtonElement;
    expect(pair.disabled).toBe(true);
    await userEvent.type(screen.getByLabelText("Computer's address"), "192.168.1.20");
    await userEvent.type(screen.getByLabelText("Code"), "482913");
    await userEvent.click(pair);
    expect(backend.last("phone_link_pair")).toMatchObject({ address: "192.168.1.20", code: "482913" });
    expect(await screen.findByText("DESK")).toBeTruthy();
  });

  test("a paired phone syncs, and shows the song being copied", async () => {
    const backend = mockBackend({ phone_link: () => paired, phone_sync_now: () => true });
    render(PhoneLinkSettings);
    await settle();
    expect(screen.getByText(/120 songs from the computer/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Sync now" }));
    expect(backend.to("phone_sync_now")).toHaveLength(1);
    phoneSync.onSync(job());
    await settle();
    expect(screen.getByText(/Copying 2 of 10 songs/)).toBeTruthy();
    expect(screen.getByText("IGNITE")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(backend.last("phone_sync_pause")).toEqual({ paused: true });
  });

  test("shows nothing on the computer", async () => {
    mockBackend({ phone_link: () => ({ ...unpaired, supported: false }) });
    render(PhoneLinkSettings);
    await settle();
    expect(screen.queryByText("Sync with computer")).toBeNull();
  });
});

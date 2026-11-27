import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import PhoneSyncSettings from "#lib/components/settings/PhoneSyncSettings.svelte";
import type { PhoneKept, PhoneService, PhoneSummary } from "#lib/phone.js";
import { isKept, phoneSync } from "#lib/phoneSync.svelte.js";
import { mockBackend, settle } from "./helpers";

const kept: PhoneKept = { playlists: ["p1"], albums: ["al1"], artistNames: ["藍井エイル", "Eir Aoi"], top: 0 };

const off: PhoneService = { enabled: false, on: false, address: "", computer: "DESK", code: null, codeUntil: 0, phones: [], error: "" };
const on: PhoneService = { ...off, enabled: true, on: true, address: "192.168.1.20:47300" };
const summary: PhoneSummary = {
  choice: { playlists: ["p1"], albums: ["al1", "al2"], artists: [], top: 100, phones: [] },
  songs: 340,
  bytes: 12_400_000_000,
  pending: 12,
  leftOut: 0,
  notSentPlaylists: 0,
};

describe("what's kept on the phone", () => {
  test("an artist is kept under any of its library names, ignoring case", () => {
    expect(isKept(kept, "artist", "eir aoi")).toBe(true);
    expect(isKept(kept, "artist", "藍井エイル")).toBe(true);
    expect(isKept(kept, "artist", "LiSA")).toBe(false);
  });

  test("playlists and albums are kept by id, and nothing is kept before it loads", () => {
    expect(isKept(kept, "playlist", "p1")).toBe(true);
    expect(isKept(kept, "album", "al2")).toBe(false);
    expect(isKept(null, "album", "al1")).toBe(false);
  });

  test("the menu item offers the opposite of what's kept, and saves it", async () => {
    const backend = mockBackend({ phone_kept: () => kept });
    await phoneSync.refresh();
    const [item] = phoneSync.menuItem("album", "al1");
    expect(item.label).toBe("Remove from Phone");
    item.action();
    await settle();
    expect(backend.last("phone_keep")).toEqual({ kind: "album", id: "al1", keep: false });
    expect(phoneSync.menuItem("album", "al9")[0].label).toBe("Keep on Phone");
  });
});

describe("Settings › Sync with phone", () => {
  function open(service: PhoneService) {
    const backend = mockBackend({
      phone_service: () => service,
      phone_summary: () => summary,
      set_phone_sync: (a) => (a.enabled ? on : off),
      phone_pair: () => ({ ...on, code: "482913", codeUntil: 1_900_000_000 }),
    });
    render(PhoneSyncSettings);
    return backend;
  }

  test("shows what goes on the phone and what is still being prepared", async () => {
    open(off);
    await settle();
    expect(screen.getByTestId("phone-total").textContent).toMatch(/340 songs, 12\.4 GB/);
    expect(screen.getByText(/12 songs still being prepared/)).toBeTruthy();
    expect(screen.getByText(/1 playlist, 2 albums, 0 artists/)).toBeTruthy();
  });

  test("turning it on shows the address, and pairing shows the code", async () => {
    const backend = open(off);
    await settle();
    expect(screen.queryByText(/Pair a phone/)).toBeNull();
    await userEvent.click(screen.getByRole("switch", { name: "Let my phone sync from this computer" }));
    expect(backend.last("set_phone_sync")).toEqual({ enabled: true });
    expect(await screen.findByText("192.168.1.20:47300")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Pair a phone" }));
    expect(await screen.findByText("482913")).toBeTruthy();
  });

  test("choosing the most played songs saves the count", async () => {
    const backend = open(off);
    await settle();
    await userEvent.click(screen.getByRole("radio", { name: "Top 200" }));
    expect(backend.last("phone_set_top")).toEqual({ top: 200 });
  });
});

// Volume and sound changes carry the output-device generation the window was showing, so the
// engine can drop one meant for a device that has since been switched away from.
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { DspSettings, Status } from "#lib/api.js";
import PlayerBar from "#lib/components/PlayerBar.svelte";
import { app } from "#lib/store.svelte.js";
import { makeLibrary, makeStatus, mockBackend, resetApp, settle } from "./helpers";

const playerStatus = (s: Status) => (app as unknown as { applyStatus(s: Status): void }).applyStatus(s);

function dsp(preamp: number): DspSettings {
  return {
    eq: { enabled: true, preamp } as unknown as DspSettings["eq"],
    leveling: { mode: "auto", preamp: 0 },
    crossfeed: { enabled: false, level: "default" },
    exclusive: false,
    crossfade: { enabled: true, seconds: 4 },
    tone: { loudness: false, smart: false },
    vocalCut: 0,
  };
}

afterEach(() => vi.useRealTimers());

describe("Volume carries the shown device generation", () => {
  async function bar(over: Partial<Status>) {
    const backend = mockBackend();
    const lib = makeLibrary(3);
    resetApp({ ...lib, status: makeStatus({ volume: 0.3, queue: lib.tracks.map((t) => t.id), index: 0, ...over }) });
    render(PlayerBar, { placement: "top" });
    await settle();
    return { backend, user: userEvent.setup() };
  }

  test("a volume change sends the generation the window shows", async () => {
    const { backend, user } = await bar({ deviceGen: 7 });
    await user.click(screen.getByRole("button", { name: "Mute" }));
    expect(backend.last("set_volume")).toMatchObject({ volume: 0, deviceGen: 7 });
  });

  test("after a status with a new generation arrives, the next change sends the new one", async () => {
    const { backend, user } = await bar({ deviceGen: 7 });
    await user.click(screen.getByRole("button", { name: "Mute" }));
    app.status = makeStatus({ ...app.status!, volume: 0, deviceGen: 8 });
    await settle();
    await user.click(screen.getByRole("button", { name: "Unmute" }));
    expect(backend.to("set_volume").map((c) => c.args.deviceGen)).toEqual([7, 8]);
  });

  test("a status without the field (older shape) sends no generation", async () => {
    const { backend, user } = await bar({ deviceGen: undefined });
    await user.click(screen.getByRole("button", { name: "Mute" }));
    expect(backend.last("set_volume")).toBeDefined();
    expect("deviceGen" in backend.last("set_volume")!).toBe(false);
  });
});

describe("Sound changes carry the generation of the edit, not of the send", () => {
  test("an edit made under generation N that fires after N+1 arrived still sends N", async () => {
    vi.useFakeTimers();
    const backend = mockBackend();
    resetApp();
    app.dsp = dsp(3);
    playerStatus(makeStatus({ deviceGen: 3 }));
    app.updateDsp((d) => (d.eq.preamp = 5));
    // Same device profile, but the output mode changed before the 40 ms send.
    playerStatus(makeStatus({ deviceGen: 4 }));
    await vi.advanceTimersByTimeAsync(100);
    expect(backend.to("set_dsp")).toHaveLength(1);
    expect(backend.last("set_dsp")?.deviceGen).toBe(3);
  });

  test("a batch of edits keeps the generation of its first edit", async () => {
    vi.useFakeTimers();
    const backend = mockBackend();
    resetApp();
    app.dsp = dsp(3);
    playerStatus(makeStatus({ deviceGen: 3 }));
    app.updateDsp((d) => (d.eq.preamp = 5));
    playerStatus(makeStatus({ deviceGen: 4 }));
    app.updateDsp((d) => (d.eq.preamp = 6));
    await vi.advanceTimersByTimeAsync(100);
    expect(backend.last("set_dsp")?.deviceGen).toBe(3);
  });

  test("the next edit after the new generation arrived sends the new one", async () => {
    vi.useFakeTimers();
    const backend = mockBackend();
    resetApp();
    app.dsp = dsp(3);
    playerStatus(makeStatus({ deviceGen: 3 }));
    app.updateDsp((d) => (d.eq.preamp = 5));
    await vi.advanceTimersByTimeAsync(100);
    playerStatus(makeStatus({ deviceGen: 4 }));
    app.updateDsp((d) => (d.eq.preamp = 6));
    await vi.advanceTimersByTimeAsync(100);
    expect(backend.to("set_dsp").map((c) => c.args.deviceGen)).toEqual([3, 4]);
  });

  test("a status without the field sends no generation", async () => {
    vi.useFakeTimers();
    const backend = mockBackend();
    resetApp();
    app.dsp = dsp(3);
    playerStatus(makeStatus({ deviceGen: undefined }));
    app.updateDsp((d) => (d.eq.preamp = 5));
    await vi.advanceTimersByTimeAsync(100);
    expect("deviceGen" in backend.last("set_dsp")!).toBe(false);
  });

  test("a device change during a drag still drops the waiting edit", async () => {
    vi.useFakeTimers();
    const backend = mockBackend({ get_dsp: () => dsp(-2) });
    resetApp();
    app.dsp = dsp(3);
    playerStatus(makeStatus({ deviceGen: 3, device: { id: "dac", name: "dac", chosen: true, profile: 1 } }));
    app.updateDsp((d) => (d.eq.preamp = 5));
    playerStatus(makeStatus({ deviceGen: 4, device: { id: "hdmi", name: "hdmi", chosen: true, profile: 1 } }));
    await vi.advanceTimersByTimeAsync(100);
    expect(backend.to("set_dsp")).toHaveLength(0);
  });
});

// The volume slider across a device switch: a drag keeps the generation it started under and ends
// when another device's status arrives, and Unmute never brings back another device's level.
import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { Status } from "#lib/api.js";
import PlayerBar from "#lib/components/PlayerBar.svelte";
import { app } from "#lib/store.svelte.js";
import { SAFE_SLIDER, VolumeControl } from "#lib/volumeControl.js";
import { makeLibrary, makeStatus, mockBackend, resetApp, settle } from "./helpers";

async function bar(over: Partial<Status>) {
  const backend = mockBackend();
  const lib = makeLibrary(3);
  resetApp({ ...lib, status: makeStatus({ queue: lib.tracks.map((t) => t.id), index: 0, ...over }) });
  render(PlayerBar, { placement: "top" });
  await settle();
  return { backend, slider: screen.getByRole("slider", { name: "Volume" }) as HTMLInputElement };
}

/** Playback moved to the DAC, held to the start level there. */
async function movedToTheDac(gen: number) {
  app.status = makeStatus({ ...app.status!, volume: 0.09, deviceGen: gen });
  await settle();
}

const sent = (b: ReturnType<typeof mockBackend>) => b.to("set_volume").map((c) => c.args);

describe("Dragging the volume slider across a device switch", () => {
  test("every step of a drag carries the generation it started under", async () => {
    const { backend, slider } = await bar({ volume: 0.25, deviceGen: 4 });
    await fireEvent.pointerDown(slider);
    await fireEvent.input(slider, { target: { value: "0.6" } });
    // A status of the same device (the engine's echo) doesn't change the stamp.
    app.status = makeStatus({ ...app.status!, volume: 0.36, deviceGen: 4 });
    await settle();
    await fireEvent.input(slider, { target: { value: "0.7" } });
    expect(sent(backend).map((a) => a.deviceGen)).toEqual([4, 4]);
  });

  test("a new device's status ends the drag: the slider shows its volume and the rest isn't sent", async () => {
    const { backend, slider } = await bar({ volume: 0.81, deviceGen: 4 });
    await fireEvent.pointerDown(slider);
    await fireEvent.input(slider, { target: { value: "0.95" } });
    await movedToTheDac(5);
    expect(+slider.value).toBeCloseTo(0.3, 5);
    // The pointer is still down and moving: nothing reaches the DAC.
    await fireEvent.input(slider, { target: { value: "0.9" } });
    expect(sent(backend)).toHaveLength(1);
    expect(sent(backend)[0]).toMatchObject({ deviceGen: 4 });
    expect(+slider.value).toBeCloseTo(0.3, 5);
    // Let go and dragged again: for the DAC now.
    await fireEvent.pointerUp(slider);
    await fireEvent.pointerDown(slider);
    await fireEvent.input(slider, { target: { value: "0.4" } });
    expect(sent(backend).at(-1)).toMatchObject({ deviceGen: 5 });
  });

  test("the keys on the focused slider send the generation of the volume shown", async () => {
    const { backend, slider } = await bar({ volume: 0.25, deviceGen: 4 });
    await movedToTheDac(9);
    await fireEvent.input(slider, { target: { value: "0.31" } });
    expect(sent(backend).at(-1)).toMatchObject({ deviceGen: 9 });
  });
});

describe("Unmute after a device switch", () => {
  test("Unmute after playback moved does not send the old device's level", async () => {
    const { backend } = await bar({ volume: 0.9, deviceGen: 4 });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Mute" }));
    app.status = makeStatus({ ...app.status!, volume: 0, deviceGen: 4 });
    await settle();
    // Moved to the DAC while muted (its status says 0 too).
    app.status = makeStatus({ ...app.status!, volume: 0, deviceGen: 5 });
    await settle();
    await user.click(screen.getByRole("button", { name: "Unmute" }));
    const last = sent(backend).at(-1)!;
    expect(last.deviceGen).toBe(5);
    expect(last.volume as number).toBeCloseTo(SAFE_SLIDER * SAFE_SLIDER, 5);
  });

  test("a new device with a volume of its own: Unmute returns to that one", () => {
    const vc = new VolumeControl();
    vc.status(0.81, 1);
    vc.press(0);
    vc.status(0.04, 2);
    expect(vc.lastNonZero).toBeCloseTo(0.2, 5);
  });
});

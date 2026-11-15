import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import PlayerBar from "#lib/components/PlayerBar.svelte";
import { app } from "#lib/store.svelte.js";
import { makeLibrary, makeStatus, mockBackend, resetApp, settle } from "./helpers";

// The volume slider is perceptual: the engine's volume is the slider's value squared.
const volumes = (b: ReturnType<typeof mockBackend>) => b.to("set_volume").map((c) => c.args.volume as number);

describe("PlayerBar volume: Mute / Unmute", () => {
  test("Unmute returns to the volume that was playing (0.3), not full scale", async () => {
    const backend = mockBackend();
    const lib = makeLibrary(3);
    resetApp({ ...lib, status: makeStatus({ volume: 0.3, queue: lib.tracks.map((t) => t.id), index: 0, duration: 200 }) });
    render(PlayerBar, { placement: "top" });
    await settle();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Mute" }));
    expect(volumes(backend)).toEqual([0]);

    // The engine reports the new volume back, as it does after every change.
    app.status = makeStatus({ ...app.status!, volume: 0 });
    await settle();

    await user.click(screen.getByRole("button", { name: "Unmute" }));
    const [muted, restored] = volumes(backend);
    expect(muted).toBe(0);
    expect(restored).toBeCloseTo(0.3, 5);
  });

  test("Unmute works without the engine's echo too (mute then unmute at once)", async () => {
    const backend = mockBackend();
    const lib = makeLibrary(3);
    resetApp({ ...lib, status: makeStatus({ volume: 0.3, queue: lib.tracks.map((t) => t.id), index: 0 }) });
    render(PlayerBar, { placement: "top" });
    await settle();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Mute" }));
    await user.click(screen.getByRole("button", { name: "Unmute" }));

    expect(volumes(backend).at(-1)).toBeCloseTo(0.3, 5);
  });

  test("an app that starts muted (volume 0): Unmute does not jump to full volume", async () => {
    const backend = mockBackend();
    const lib = makeLibrary(3);
    resetApp({ ...lib, status: makeStatus({ volume: 0, queue: lib.tracks.map((t) => t.id), index: 0 }) });
    render(PlayerBar, { placement: "top" });
    await settle();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Unmute" }));

    const [v] = volumes(backend);
    expect(v).toBeGreaterThan(0);
    expect(v).toBeLessThan(1);
  });
});

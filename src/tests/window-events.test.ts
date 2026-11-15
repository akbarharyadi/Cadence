// Events for one window must not reach the other: a global `listen` hears `emit_to` aimed at any
// window, so the mini player froze its bars when the main window was minimized. Tauri's mock can't
// route a targeted emit, so these check what the page registers with the backend instead.
import { beforeEach, describe, expect, it } from "vitest";
import { app } from "#lib/store.svelte.js";
import { makeLibrary, makeStatus, mockBackend, resetApp } from "./helpers";

type Listen = { event: string; target: { kind: string; label?: string } };

/** Every `plugin:event|listen` the page makes, as the backend would see it. */
function watchListens(): Listen[] {
  const seen: Listen[] = [];
  const internals = (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ as { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
  const real = internals.invoke;
  internals.invoke = (cmd, args) => {
    if (cmd === "plugin:event|listen") seen.push({ event: args?.event as string, target: args?.target as Listen["target"] });
    return real(cmd, args);
  };
  return seen;
}

function boundTo(seen: Listen[], event: string) {
  return seen.filter((l) => l.event === event).map((l) => l.target);
}

describe("window events", () => {
  beforeEach(() => resetApp());

  it("the main window hears its own player updates and visibility, and everything else app-wide", async () => {
    const lib = makeLibrary(2);
    mockBackend({ get_library: () => lib, get_status: () => makeStatus() }, "main");
    const seen = watchListens();
    await app.init();
    expect(boundTo(seen, "player-status")).toEqual([{ kind: "WebviewWindow", label: "main" }]);
    expect(boundTo(seen, "window-visible")).toEqual([{ kind: "WebviewWindow", label: "main" }]);
    expect(boundTo(seen, "library-changed")).toEqual([{ kind: "Any" }]);
    expect(boundTo(seen, "sleep-timer")).toEqual([{ kind: "Any" }]);
    expect(boundTo(seen, "analysis-progress")).toEqual([{ kind: "Any" }]);
  });

  it("the mini player listens to its own window only, so the main window's minimizing is not its own", async () => {
    const lib = makeLibrary(2);
    mockBackend({ get_library: () => lib, get_library_part: () => lib, get_status: () => makeStatus() }, "mini");
    const seen = watchListens();
    await app.initMini();
    expect(boundTo(seen, "player-status")).toEqual([{ kind: "WebviewWindow", label: "mini" }]);
    expect(boundTo(seen, "window-visible")).toEqual([{ kind: "WebviewWindow", label: "mini" }]);
    expect(boundTo(seen, "library-changed")).toEqual([{ kind: "Any" }]);
  });
});

// Background jobs' progress bars: a library-wide job runs for hours, so its bar must cost nothing
// between updates. Only `transform` moves it (no layout), with no transition that a steady
// stream of updates would keep restarting, and an update changes the bar and nothing else.
import { describe, expect, test } from "vitest";
import { render } from "@testing-library/svelte";
import { emit } from "@tauri-apps/api/event";
import Sidebar from "#lib/components/Sidebar.svelte";
import { app } from "#lib/store.svelte.js";
import { makeLibrary, mockBackend, resetApp, settle } from "./helpers";

async function sources(): Promise<{ file: string; text: string }[]> {
  const { readdirSync, readFileSync, statSync } = await import("node:fs");
  const { join } = await import("node:path");
  const out: { file: string; text: string }[] = [];
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      if (statSync(p).isDirectory()) {
        if (name !== "tests") walk(p);
      } else if (/\.(svelte|css)$/.test(name)) out.push({ file: p, text: readFileSync(p, "utf8") });
    }
  };
  walk(join(process.cwd(), "src"));
  return out;
}

describe("Progress bars", () => {
  test("where jobs show progress, nothing transitions or animates a size or place (each update would lay the page out again)", async () => {
    const files = await sources();
    const shows = /[\\/](Sidebar|Settings|UpdateBanner|Welcome|ProgressBar)\.svelte$/;
    const checked = files.filter((f) => shows.test(f.file));
    expect(checked.length).toBe(5);
    const layout = /(transition|transition-property|animation)\s*:[^;]*\b(width|height|left|top|min-width|max-width)\b/;
    expect(checked.filter(({ text }) => layout.test(text)).map((f) => f.file)).toEqual([]);
  });

  test("every progress bar is the shared one, and it neither transitions nor animates", async () => {
    const files = await sources();
    const own = files.filter(({ text }) => /role="progressbar"/.test(text)).map((f) => f.file.replace(/\\/g, "/"));
    expect(own).toEqual([expect.stringMatching(/components\/ui\/ProgressBar\.svelte$/)]);
    const bar = files.find((f) => /components[\\/]ui[\\/]ProgressBar\.svelte$/.test(f.file))!.text;
    expect(bar).not.toMatch(/\b(transition|animation)\s*:/);
    // Nothing sizes a bar from a job's numbers.
    const sized = files.filter(({ text }) => /style="width:\s*\{[^}]*(done|total|analyzed|installing|progress)/.test(text)).map((f) => f.file);
    expect(sized).toEqual([]);
  });

  test("the sidebar's job bar moves by transform only", async () => {
    mockBackend();
    resetApp(makeLibrary(3));
    app.analysis = { done: 25, total: 100 };
    const { container } = render(Sidebar);
    await settle();
    const fill = container.querySelector<HTMLElement>("[data-progress-fill]")!;
    expect(fill.getAttribute("style")).toMatch(/^transform: scaleX\(0\.25\);?$/);
    app.analysis = { done: 50, total: 100 };
    await settle();
    expect(fill.getAttribute("style")).toMatch(/^transform: scaleX\(0\.5\);?$/);
    expect(container.querySelector<HTMLElement>("[role=progressbar]")!.style.width).toBe("");
    app.analysis = null;
  });

  test("a job's progress updates its bar without reloading or replacing the library", async () => {
    const { tracks, albums } = makeLibrary(50);
    const backend = mockBackend({
      get_library: () => ({ tracks, albums }),
      get_folders: () => [],
      get_history: () => ({ plays: {}, skips: {}, lastPlayed: {}, recentAlbums: [], totalListened: 0 }),
      get_playlists: () => [],
      get_status: () => null,
      sonic_status: () => ({ analyzed: 0, total: 50, running: true }),
    });
    resetApp();
    await app.init().catch(() => {});
    await settle();
    const libraryCalls = backend.to("get_library").length;
    const [shownTracks, shownAlbums] = [app.tracks, app.albums];
    for (let done = 1; done < 50; done++) {
      await emit("analysis-progress", { done, total: 50 });
      await emit("sonic-progress", { done, total: 50 });
    }
    await settle();
    expect(app.analysis).toEqual({ done: 49, total: 50 });
    expect(backend.to("get_library").length).toBe(libraryCalls);
    expect(app.tracks).toBe(shownTracks);
    expect(app.albums).toBe(shownAlbums);
  });
});

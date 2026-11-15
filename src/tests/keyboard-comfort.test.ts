import { cleanup, render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockWindows } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import AppKeys from "#lib/components/AppKeys.svelte";
import Settings from "#lib/views/Settings.svelte";
import Search from "#lib/views/Search.svelte";
import Sidebar from "#lib/components/Sidebar.svelte";
import { overlays } from "#lib/palette.svelte.js";
import { ALL_SHORTCUTS, SHORTCUT_GROUPS } from "#lib/shortcuts.js";
import { app } from "#lib/store.svelte.js";
import { applyZoom, savedZoom, uiZoom, ZOOM_KEY } from "#lib/uiZoom.svelte.js";
import { makeLibrary, mockBackend, resetApp } from "./helpers";

let user: ReturnType<typeof userEvent.setup>;

beforeEach(() => {
  mockBackend({
    get_app_settings: () => ({ closeToTray: true, globalHotkeys: true, lyricsOnline: true, fixTagsOnline: true, listenbrainzUser: "" }),
  });
  user = userEvent.setup();
  overlays.paletteOpen = false;
  overlays.helpOpen = false;
});
afterEach(() => {
  cleanup();
  overlays.paletteOpen = false;
  overlays.helpOpen = false;
  uiZoom.reset();
});

describe("search shortcuts", () => {
  test("Ctrl+F focuses the sidebar's search box and selects its text", async () => {
    resetApp(makeLibrary(3));
    app.search = "yonezu";
    render(Sidebar);
    render(AppKeys);
    const input = screen.getByPlaceholderText("Search your music") as HTMLInputElement;
    expect(input).not.toHaveFocus();

    await user.keyboard("{Control>}f{/Control}");
    expect(input).toHaveFocus();
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 6]);
  });

  test("'/' does the same when nothing is being typed, and is just a slash inside a text box", async () => {
    resetApp(makeLibrary(3));
    render(Sidebar);
    render(AppKeys);
    const input = screen.getByPlaceholderText("Search your music") as HTMLInputElement;

    await user.keyboard("/");
    expect(input).toHaveFocus();
    expect(input).toHaveValue(""); // the key opened the box; it didn't type itself into it

    // In a text box the key types. (Another field: the settings' filter.)
    await user.type(input, "AC/DC");
    expect(input).toHaveValue("AC/DC");
  });

  test("on a phone (no sidebar) it takes the Search page's own box", async () => {
    resetApp(makeLibrary(3));
    render(Search, { query: "" });
    render(AppKeys);
    const input = screen.getByRole("searchbox", { name: "Search your music" });
    await user.keyboard("{Control>}f{/Control}");
    expect(input).toHaveFocus();
  });

  test("with no search box on screen it opens the Search page", async () => {
    resetApp(makeLibrary(3));
    render(AppKeys);
    await user.keyboard("{Control>}f{/Control}");
    await waitFor(() => expect(app.view.name).toBe("search"));
  });

  test("the shortcut keys stay out of the way while the palette is open", async () => {
    resetApp(makeLibrary(3));
    render(Sidebar);
    render(AppKeys);
    await user.keyboard("{Control>}k{/Control}");
    await user.keyboard("{Control>}f{/Control}");
    expect(screen.getByPlaceholderText("Search your music")).not.toHaveFocus();
  });
});

describe("keyboard shortcut help", () => {
  test("'?' opens the list, which has every shortcut of the one list; Escape closes it", async () => {
    resetApp(makeLibrary(3));
    render(AppKeys);
    await user.keyboard("?");
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    expect(dialog).toBeInTheDocument();
    for (const s of ALL_SHORTCUTS) expect(dialog).toHaveTextContent(s.action.slice(0, 20));
    for (const g of SHORTCUT_GROUPS) expect(dialog).toHaveTextContent(g.title);

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  test("'?' typed in a text box types a question mark", async () => {
    resetApp(makeLibrary(3));
    render(Sidebar);
    render(AppKeys);
    const input = screen.getByPlaceholderText("Search your music");
    await user.type(input, "why?");
    expect(input).toHaveValue("why?");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  test("Settings has a button for it", async () => {
    resetApp(makeLibrary(3));
    render(Settings);
    render(AppKeys);
    await user.click(screen.getByRole("button", { name: /Show shortcuts/ }));
    expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
  });
});

describe("Settings search", () => {
  const sectionTitles = () => Array.from(document.querySelectorAll<HTMLElement>("section:not([hidden]) h2")).map((h) => h.textContent?.trim());

  test("typing hides what doesn't match, and clearing brings everything back", async () => {
    resetApp(makeLibrary(3));
    render(Settings);
    const box = screen.getByRole("searchbox", { name: "Search settings" });
    const all = sectionTitles();
    expect(all).toContain("Music folders");
    expect(all).toContain("Updates");

    await user.type(box, "updates");
    await waitFor(() => expect(sectionTitles()).not.toContain("Music folders"));
    expect(sectionTitles()).toContain("Updates");
    expect(document.querySelector("section[hidden]")).not.toBeNull();

    await user.clear(box);
    await waitFor(() => expect(sectionTitles()).toEqual(all));
    expect(document.querySelectorAll("[hidden]")).toHaveLength(0);
  });

  test("matches the description as well as the title, ignoring case and accents", async () => {
    resetApp(makeLibrary(3));
    render(Settings);
    // Only the description of "Find lyrics online" says LRCLIB.
    await user.type(screen.getByRole("searchbox", { name: "Search settings" }), "LRCLÍB");
    await waitFor(() => expect(sectionTitles()).not.toContain("Music folders"));
    expect(sectionTitles()).toContain("Find lyrics online");
  });

  test("in Appearance only the matching row stays", async () => {
    resetApp(makeLibrary(3));
    render(Settings);
    await user.type(screen.getByRole("searchbox", { name: "Search settings" }), "interface size");
    await waitFor(() => expect(sectionTitles()).not.toContain("Music folders"));
    const rows = Array.from(document.querySelectorAll<HTMLElement>("section:not([hidden]) .title-row:not([hidden]) h2")).map((h) => h.textContent?.trim());
    expect(rows).toContain("Interface size");
    expect(rows).not.toContain("Player position");
  });

  test("nothing matching says so; Sound page settings show as links to it", async () => {
    resetApp(makeLibrary(3));
    render(Settings);
    const box = screen.getByRole("searchbox", { name: "Search settings" });
    await user.type(box, "qqqzzz");
    expect(await screen.findByText(/No matching settings/)).toBeInTheDocument();

    await user.clear(box);
    await user.type(box, "crossfade");
    const link = await screen.findByRole("button", { name: /Crossfade/ });
    expect(screen.queryByText(/No matching settings/)).not.toBeInTheDocument();
    await user.click(link);
    expect(app.view).toEqual({ name: "sound" });
  });

  test("Escape empties the box first", async () => {
    resetApp(makeLibrary(3));
    render(Settings);
    const box = screen.getByRole("searchbox", { name: "Search settings" });
    await user.type(box, "updates");
    await user.keyboard("{Escape}");
    expect(box).toHaveValue("");
  });
});

describe("interface size", () => {
  const ZOOM_CMD = "plugin:webview|set_webview_zoom";
  const settle = () => new Promise((r) => setTimeout(r, 0));
  let be: ReturnType<typeof mockBackend>;
  const zooms = () => be.to(ZOOM_CMD).map((c) => c.args.value);

  beforeEach(() => {
    be = mockBackend({
      get_app_settings: () => ({ closeToTray: true, globalHotkeys: true, lyricsOnline: true, fixTagsOnline: true, listenbrainzUser: "" }),
      [ZOOM_CMD]: () => null,
    });
    mockWindows("main");
  });

  test("Ctrl+= and Ctrl+- step through 90, 100, 110, 125, 150 %; Ctrl+0 resets; each is an absolute native zoom and is saved", async () => {
    render(AppKeys);
    expect(uiZoom.value).toBe(1);
    expect(zooms()).toEqual([]); // 100% leaves the webview alone

    await user.keyboard("{Control>}={/Control}");
    expect(uiZoom.value).toBe(1.1);
    expect(be.last(ZOOM_CMD)).toEqual({ label: "main", value: 1.1 });
    expect(localStorage.getItem(ZOOM_KEY)).toBe("1.1");

    await user.keyboard("{Control>}={/Control}{Control>}={/Control}{Control>}={/Control}");
    expect(uiZoom.value).toBe(1.5); // the top step
    await user.keyboard("{Control>}-{/Control}");
    expect(uiZoom.value).toBe(1.25);

    await user.keyboard("{Control>}0{/Control}");
    expect(uiZoom.value).toBe(1);
    expect(zooms().at(-1)).toBe(1);
    expect(localStorage.getItem(ZOOM_KEY)).toBeNull();

    await user.keyboard("{Control>}-{/Control}{Control>}-{/Control}");
    expect(uiZoom.value).toBe(0.9); // the bottom step
    expect(zooms().at(-1)).toBe(0.9);
    expect(localStorage.getItem(ZOOM_KEY)).toBe("0.9");
    expect(document.documentElement.style.zoom).toBe(""); // no CSS zoom workaround any more
  });

  test("works from a text box too, and Ctrl++ (Shift on the = key) counts as larger", async () => {
    resetApp(makeLibrary(3));
    render(Sidebar);
    render(AppKeys);
    await user.click(screen.getByPlaceholderText("Search your music"));
    await user.keyboard("{Control>}{Shift>}+{/Shift}{/Control}");
    expect(uiZoom.value).toBe(1.1);
  });

  test("a saved size is applied when the page starts (a bad value is ignored)", async () => {
    localStorage.setItem(ZOOM_KEY, "1.25");
    expect(savedZoom()).toBe(1.25);
    expect(await applyZoom(savedZoom())).toBe(true);
    expect(be.last(ZOOM_CMD)).toEqual({ label: "main", value: 1.25 });

    localStorage.setItem(ZOOM_KEY, "banana");
    expect(savedZoom()).toBe(1);
    localStorage.setItem(ZOOM_KEY, "1.27"); // between steps: the nearest one
    expect(savedZoom()).toBe(1.25);
  });

  test("when the native zoom is refused (no permission, unsupported platform) the size falls back to 100% without throwing", async () => {
    be = mockBackend({
      get_app_settings: () => ({ closeToTray: true, globalHotkeys: true, lyricsOnline: true, fixTagsOnline: true, listenbrainzUser: "" }),
      [ZOOM_CMD]: () => {
        throw "not allowed";
      },
    });
    mockWindows("main");
    expect(await applyZoom(1.25)).toBe(false);
    be.calls.length = 0;

    render(AppKeys);
    await user.keyboard("{Control>}={/Control}");
    await settle();
    expect(be.to(ZOOM_CMD)).toHaveLength(1);
    expect(uiZoom.value).toBe(1);
  });

  test("Settings has the choices, and picking one applies and saves it", async () => {
    resetApp(makeLibrary(3));
    render(Settings);
    const group = screen.getByRole("radiogroup", { name: "Interface size" });
    expect(group.querySelectorAll("[role=radio]")).toHaveLength(5);
    await user.click(screen.getByRole("radio", { name: "125%" }));
    expect(uiZoom.value).toBe(1.25);
    expect(be.last(ZOOM_CMD)).toEqual({ label: "main", value: 1.25 });
    expect(localStorage.getItem(ZOOM_KEY)).toBe("1.25");
    expect(screen.getByRole("radio", { name: "125%" })).toHaveAttribute("aria-checked", "true");
  });
});

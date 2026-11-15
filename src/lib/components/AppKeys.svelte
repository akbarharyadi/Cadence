<script lang="ts">
  // The window's finding-and-comfort shortcuts, and the two overlays they open:
  //   Ctrl+K command palette, Ctrl+F or "/" search, "?" shortcut help, Ctrl+= / Ctrl+- / Ctrl+0 interface size,
  //   and Escape clearing a song selection when nothing above the page takes it.
  // (Playing and navigation keys stay in routes/+page.svelte.) The keys are listed in shortcuts.ts.
  import { focusSearch } from "#lib/focusSearch.js";
  import { openHelp, openPalette, overlays } from "#lib/palette.svelte.js";
  import { app } from "#lib/store.svelte.js";
  import { leaveSelection } from "#lib/selection.js";
  import { uiZoom } from "#lib/uiZoom.svelte.js";
  import CommandPalette from "./CommandPalette.svelte";
  import Guard from "./Guard.svelte";
  import ShortcutHelp from "./ShortcutHelp.svelte";

  function typingIn(target: HTMLElement) {
    return (
      (target.tagName === "INPUT" && !["range", "checkbox", "radio", "button"].includes((target as HTMLInputElement).type)) ||
      target.tagName === "TEXTAREA" ||
      target.tagName === "SELECT" ||
      target.isContentEditable
    );
  }

  /** The top-most open dialog (the palette, help and every Modal are aria-modal), if any. */
  function topDialog(): HTMLElement | null {
    const all = document.querySelectorAll<HTMLElement>("[aria-modal='true']");
    return all.length ? all[all.length - 1] : null;
  }

  /**
   * Ctrl+F and "/": the search box, unless something covers the page. A dialog with a search box
   * of its own (the palette) gets that one; any other dialog, a context menu or Now Playing keeps
   * the key (the box behind it would turn the page under the dialog into Search).
   */
  function findKey(): boolean {
    const dialog = topDialog();
    if (dialog) {
      const own = dialog.querySelector<HTMLInputElement>("input[type='search'], input[role='combobox'], input[role='searchbox']");
      if (!own) return false;
      own.focus();
      own.select?.();
      return true;
    }
    if (app.menu || app.nowPlayingOpen || document.querySelector("[role=menu]")) return false;
    void focusSearch();
    return true;
  }

  function onKey(e: KeyboardEvent) {
    const target = (e.target instanceof HTMLElement ? e.target : document.body) as HTMLElement;
    const mod = (e.ctrlKey || e.metaKey) && !e.altKey;
    const key = e.key;

    if (mod && !e.shiftKey && key.toLowerCase() === "k") {
      e.preventDefault();
      if (overlays.paletteOpen) overlays.paletteOpen = false;
      else {
        app.menu = null;
        openPalette();
      }
      return;
    }
    if (mod && !e.shiftKey && key.toLowerCase() === "f") {
      e.preventDefault();
      findKey();
      return;
    }
    // Interface size: works from a text box too. (Shift+= types "+" on most keyboards.)
    if (mod && !e.defaultPrevented) {
      const step = key === "=" || key === "+" ? 1 : key === "-" || key === "_" ? -1 : 0;
      if (step || key === "0") {
        e.preventDefault();
        if (step) uiZoom.step(step as 1 | -1);
        else uiZoom.reset();
        return;
      }
    }

    // "/" and "?" are letters in a text box.
    if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || typingIn(target)) return;
    // Escape, last in line: what's above the page (a menu, a dialog, the palette, Now Playing)
    // takes it first, and a song list with the keyboard clears its own selection. Otherwise a
    // multi-selection left behind (focus moved to the bar, "Add to Playlist…") is cleared here.
    // It never navigates.
    if (key === "Escape") {
      if (e.shiftKey || app.menu || app.nowPlayingOpen || overlays.paletteOpen || overlays.helpOpen || document.querySelector("[aria-modal='true'], [role=menu]")) return;
      if (leaveSelection()) e.preventDefault();
      return;
    }
    // An open dialog, menu or the palette has the keyboard.
    if (key === "/") {
      // Never typed into the page under a dialog; a dialog's own search box takes it (see findKey).
      if (!overlays.helpOpen && findKey()) e.preventDefault();
      return;
    }
    if (overlays.paletteOpen || overlays.helpOpen || app.menu || topDialog() || target.closest("[aria-modal='true'], [role=menu]")) return;
    if (key === "?") {
      e.preventDefault();
      openHelp();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if overlays.paletteOpen}
  <Guard label="The command palette" overlay onclose={() => (overlays.paletteOpen = false)}><CommandPalette /></Guard>
{/if}
{#if overlays.helpOpen}
  <Guard label="Keyboard shortcuts" overlay onclose={() => (overlays.helpOpen = false)}><ShortcutHelp /></Guard>
{/if}

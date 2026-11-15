<script lang="ts">
  // The window's finding-and-comfort shortcuts, and the two overlays they open:
  //   Ctrl+K command palette, Ctrl+F or "/" search, "?" shortcut help, Ctrl+= / Ctrl+- / Ctrl+0 interface size.
  // (Playing and navigation keys stay in routes/+page.svelte.) The keys are listed in shortcuts.ts.
  import { focusSearch } from "#lib/focusSearch.js";
  import { openHelp, openPalette, overlays } from "#lib/palette.svelte.js";
  import { app } from "#lib/store.svelte.js";
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
      if (!overlays.paletteOpen && !overlays.helpOpen) focusSearch();
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
    // An open dialog, menu or the palette has the keyboard.
    if (overlays.paletteOpen || overlays.helpOpen || app.menu || target.closest("[aria-modal='true'], [role=menu]")) return;
    if (key === "/") {
      e.preventDefault();
      focusSearch();
    } else if (key === "?") {
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

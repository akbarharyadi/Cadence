// Whether the command palette and the shortcut help are open. A tiny shared state so the window's
// key handler (routes/+page.svelte) can stand aside while the palette has the keyboard.

class Overlays {
  paletteOpen = $state(false);
  helpOpen = $state(false);
}
export const overlays = new Overlays();

export function openPalette() {
  overlays.helpOpen = false;
  overlays.paletteOpen = true;
}
export function openHelp() {
  overlays.paletteOpen = false;
  overlays.helpOpen = true;
}

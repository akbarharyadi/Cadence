// Every keyboard shortcut Cadence has, in one place: the Settings list, the "?" dialog and the
// command palette (which shows the keys beside an action) all read it. When a key is added to the
// window's handler (routes/+page.svelte) or to a component, add it here too.

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** The key written for Ctrl: "Ctrl" on Windows and Linux, the command sign on a Mac. */
export const MOD = isMac ? "⌘" : "Ctrl";

export interface Shortcut {
  /** Stable name, for the palette to find the keys of its actions. */
  id: string;
  keys: string[];
  action: string;
}
export interface ShortcutGroup {
  title: string;
  items: Shortcut[];
}

export const SHORTCUT_GROUPS: ShortcutGroup[] = [
  {
    title: "Playing",
    items: [
      { id: "toggle", keys: ["Space"], action: "Play or pause" },
      { id: "next", keys: ["Ctrl", "→"], action: "Next song" },
      { id: "prev", keys: ["Ctrl", "←"], action: "Previous song" },
      { id: "forward", keys: ["→"], action: "Forward 10 seconds" },
      { id: "backward", keys: ["←"], action: "Back 10 seconds" },
      { id: "shuffle", keys: ["Ctrl", "S"], action: "Shuffle on or off" },
      { id: "repeat", keys: ["Ctrl", "R"], action: "Repeat: off, all, this song" },
      { id: "queue", keys: ["Ctrl", "U"], action: "Show or hide Up Next" },
      { id: "lyrics", keys: ["Ctrl", "L"], action: "Lyrics" },
      { id: "media", keys: ["Media keys"], action: "Play, pause, next and previous, even when Cadence is in the background" },
    ],
  },
  {
    title: "Finding things",
    items: [
      { id: "palette", keys: [MOD, "K"], action: "Command palette: pages, actions and your music" },
      { id: "search", keys: [MOD, "F"], action: "Search your music" },
      { id: "search-slash", keys: ["/"], action: "Search your music (when you’re not typing)" },
      { id: "help", keys: ["?"], action: "Keyboard shortcuts" },
    ],
  },
  {
    title: "Around the app",
    items: [
      { id: "back", keys: ["Alt", "←"], action: "Go back" },
      { id: "forward-page", keys: ["Alt", "→"], action: "Go forward" },
      { id: "zoom-in", keys: [MOD, "="], action: "Larger interface" },
      { id: "zoom-out", keys: [MOD, "-"], action: "Smaller interface" },
      { id: "zoom-reset", keys: [MOD, "0"], action: "Normal interface size" },
      { id: "escape", keys: ["Esc"], action: "Close the dialog, menu or Now Playing on top" },
    ],
  },
  {
    title: "In Needs Attention",
    items: [
      { id: "na-move", keys: ["↑", "↓"], action: "Move between cards (J and K work too)" },
      { id: "na-open", keys: ["Enter"], action: "Open the card" },
      { id: "na-ignore", keys: ["I"], action: "Ignore the card" },
      { id: "na-tick", keys: ["Space"], action: "Tick the card" },
    ],
  },
];

/** Settings lists these (the same, flat). */
export const ALL_SHORTCUTS: Shortcut[] = SHORTCUT_GROUPS.flatMap((g) => g.items);

/** The keys of one shortcut by its id, or undefined. */
export const keysOf = (id: string): string[] | undefined => ALL_SHORTCUTS.find((s) => s.id === id)?.keys;

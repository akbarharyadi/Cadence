// The pages you can open from the sidebar (and, on phones, the tab bar and its Library page).
// The command palette lists them; the playlists are added by the caller.
import type { View } from "./store.svelte.js";

export interface Place {
  label: string;
  icon: string;
  view: View;
  /** Other words that find it. */
  words?: string;
}

export const PLACES: Place[] = [
  { label: "Listen Now", icon: "home", view: { name: "home" }, words: "home start" },
  { label: "Discover", icon: "sparkle", view: { name: "discover" }, words: "recommendations new music" },
  { label: "Your Stats", icon: "gauge", view: { name: "stats" }, words: "statistics history listening" },
  { label: "Songs", icon: "song", view: { name: "songs" }, words: "tracks" },
  { label: "Albums", icon: "album", view: { name: "albums" } },
  { label: "Artists", icon: "artist", view: { name: "artists" } },
  { label: "Recently Added", icon: "clock", view: { name: "recent" }, words: "new latest" },
  { label: "Favorites", icon: "heart", view: { name: "favorites" }, words: "loved liked" },
  { label: "Needs Attention", icon: "edit", view: { name: "attention" }, words: "fix tags duplicates problems" },
  { label: "Library Health", icon: "shield", view: { name: "health" }, words: "damaged files quality check" },
  { label: "Sound", icon: "eq", view: { name: "sound" }, words: "equalizer eq crossfeed crossfade output" },
  { label: "Settings", icon: "settings", view: { name: "settings" }, words: "preferences options" },
];

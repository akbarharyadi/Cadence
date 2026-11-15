// Making the library's search index ahead of the first key press (Search and the command palette
// share it): while the window is idle, once a search box gets focus or either of them opens.
import { albumFields, artistFields, trackFields, warm } from "./searchIndex";
import { app } from "./store.svelte.js";

/** Starts making the index of the songs, albums and artists (nothing if it's made already). */
export function warmSearch() {
  warm(app.tracks, trackFields);
  warm(app.albums, albumFields);
  warm(app.artists, artistFields);
}

// A search box got focus (clicked, or Ctrl+F): the typing comes next. (Imported by the Search
// page, which the shell loads at start, so this is listening from the first screen on.)
if (typeof document !== "undefined") {
  document.addEventListener("focusin", (e) => {
    const el = e.target as Element | null;
    if (el?.matches?.("[data-global-search]")) warmSearch();
  });
}

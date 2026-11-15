// Ctrl+F and "/": the search box gets the keyboard, with its text selected.
import { tick } from "svelte";
import { app } from "./store.svelte.js";

const box = (kind: "sidebar" | "page") => document.querySelector<HTMLInputElement>(`[data-global-search="${kind}"]`);

/** The sidebar's search box (desktop), or the Search page's own (phones, which have no sidebar). */
export async function focusSearch() {
  let el = box("sidebar") ?? box("page");
  if (!el) {
    // No box on this page (a phone): open the Search page and take its input once it's there.
    app.go({ name: "search", query: "" });
    for (let tries = 0; tries < 12 && !el; tries++) {
      await tick();
      el = box("page");
      if (!el) await new Promise((r) => setTimeout(r, 40));
    }
  }
  if (!el) return;
  el.focus();
  el.select();
}

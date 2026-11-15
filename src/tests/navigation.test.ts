import { beforeEach, describe, expect, test } from "vitest";
import { app } from "#lib/store.svelte.js";
import { mockBackend, resetApp } from "./helpers";

// Back / Forward in the store. (No webview: `go` skips the view transition when the browser has none.)
beforeEach(() => {
  mockBackend(); // `go` tells the engine the UI is busy
  resetApp();
});

const name = () => app.view.name;
const flush = () => new Promise<void>((r) => setTimeout(r, 0));

describe("navigation history", () => {
  test("opening Albums three times, then Back once, leaves Albums (no duplicate entries)", () => {
    app.go({ name: "albums" });
    app.go({ name: "albums" });
    app.go({ name: "albums" });
    expect(app.history).toHaveLength(1);

    app.back();
    expect(name()).toBe("home");
    expect(app.history).toHaveLength(0);
  });

  test("Back and Forward round-trip through several pages", () => {
    app.go({ name: "albums" });
    app.go({ name: "album", id: "al1" });
    app.go({ name: "artist", artist: "Artist" });

    app.back();
    expect(app.view).toEqual({ name: "album", id: "al1" });
    app.back();
    expect(app.view).toEqual({ name: "albums" });
    app.back();
    expect(app.view).toEqual({ name: "home" });
    app.back(); // nothing left to go back to
    expect(app.view).toEqual({ name: "home" });

    app.forward();
    expect(app.view).toEqual({ name: "albums" });
    app.forward();
    expect(app.view).toEqual({ name: "album", id: "al1" });
    app.forward();
    expect(app.view).toEqual({ name: "artist", artist: "Artist" });
    app.forward(); // nothing left to go forward to
    expect(app.view).toEqual({ name: "artist", artist: "Artist" });
    expect(app.forwardHistory).toHaveLength(0);
  });

  test("the sidebar section follows Back (an album opened from Albums keeps Albums lit)", () => {
    app.go({ name: "albums" });
    app.go({ name: "album", id: "al1" });
    expect(app.section).toBe("albums");
    app.go({ name: "songs" });
    expect(app.section).toBe("songs");
    app.back();
    expect(app.section).toBe("albums");
  });

  test("history is capped at 100 pages, dropping the oldest", () => {
    for (let i = 0; i < 150; i++) app.go({ name: "album", id: `a${i}` });
    expect(app.history).toHaveLength(100);
    // 150 pages opened after Home: Home and the first 49 albums have been forgotten.
    expect(app.history[0]).toEqual({ name: "album", id: "a49" });

    for (let i = 0; i < 100; i++) app.back();
    expect(app.history).toHaveLength(0);
    expect(app.view).toEqual({ name: "album", id: "a49" });
  });

  test("the forward list is capped too", () => {
    for (let i = 0; i < 150; i++) app.go({ name: "album", id: `a${i}` });
    for (let i = 0; i < 100; i++) app.back();
    expect(app.forwardHistory.length).toBeLessThanOrEqual(100);
  });

  test("opening a new page clears Forward", () => {
    app.go({ name: "albums" });
    app.go({ name: "artists" });
    app.back();
    expect(app.forwardHistory).toHaveLength(1);

    app.go({ name: "songs" });
    expect(app.forwardHistory).toHaveLength(0);
    app.forward();
    expect(app.view).toEqual({ name: "songs" });
  });
});

// The sidebar's box owns the text: typing sets `app.search` and then opens (or refines) the results page.
const type = (q: string) => {
  app.search = q;
  app.go({ name: "search", query: q });
};

describe("the search text and navigation", () => {
  test("the sidebar's search text is cleared when you navigate away from the results", async () => {
    type("yonezu");
    expect(app.view).toEqual({ name: "search", query: "yonezu" });
    expect(app.search).toBe("yonezu");

    app.go({ name: "albums" });
    expect(app.search).toBe("");
    await flush();
    expect(app.search).toBe("");
  });

  test("Back from the results to the page before also leaves the box empty; Forward brings the text back", () => {
    app.go({ name: "albums" });
    type("lemon");
    app.back();
    expect(name()).toBe("albums");
    expect(app.search).toBe("");

    app.forward();
    expect(app.view).toEqual({ name: "search", query: "lemon" });
    expect(app.search).toBe("lemon");
  });

  test("refining the query on the search page keeps one history entry", () => {
    app.go({ name: "albums" });
    type("l");
    type("le");
    type("lem");
    expect(app.history).toHaveLength(2); // home, albums
    expect(app.search).toBe("lem");
    expect(app.view).toEqual({ name: "search", query: "lem" });
  });
});

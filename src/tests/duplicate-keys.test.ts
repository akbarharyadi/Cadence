import { render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, test } from "vitest";
import Discover from "#lib/views/Discover.svelte";
import Home from "#lib/views/Home.svelte";
import type { AiMix, NewSong } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeLibrary, makeMix, mockBackend, resetApp, settle } from "./helpers";

// In development builds (which is what runs here) Svelte throws `each_key_duplicate` when two items of
// a keyed {#each} share a key. The AI's picks and mixes are not ours to keep unique: two picks can be
// the same artist and title, two mixes can share a name.
const pick = (over: Partial<NewSong> = {}): NewSong => ({ artist: "Kenshi Yonezu", title: "Lemon", why: "Because you play him", ...over });

function aiReady() {
  app.ai = {
    hasKey: true,
    settings: { provider: "cloud", service: "claude", cloudUrl: "", cloudModel: "", claudeModel: "", webSearch: false },
  };
}

beforeEach(() => {
  mockBackend({
    listening_profile: () => ({ monthArtists: [], monthGenres: [], monthSongs: [], artists: [], genres: [] }),
    ai_recommended: () => ({ basis: "", value: "", at: 0, picks: [] }),
  });
});

describe("repeated names do not break the page", () => {
  test("Home: two AI picks with the same artist and title, two mixes with the same title", async () => {
    const lib = makeLibrary(6);
    resetApp({ tracks: lib.tracks, albums: [makeAlbum(lib.tracks)] });
    aiReady();
    const ids = lib.tracks.map((t) => t.id);
    const twin: AiMix = { title: "Late night", reason: "Calm", trackIds: ids.slice(0, 3) };
    app.aiMixes = [twin, { ...twin, reason: "Another take", trackIds: ids.slice(3) }];
    app.aiNew = [pick(), pick({ why: "Again" })];
    app.mixes = [makeMix("Mellow", ids.slice(0, 2)), makeMix("Mellow", ids.slice(2, 4))];

    expect(() => render(Home)).not.toThrow();
    await settle();

    expect(screen.getAllByText("Late night").length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText("Lemon")).toHaveLength(2);
    expect(screen.getAllByText("Mellow").length).toBeGreaterThanOrEqual(2);
  });

  test("Discover: two picks with the same artist and title", async () => {
    resetApp();
    aiReady();
    app.daily = { basis: "", value: "", at: 0, picks: [pick(), pick({ why: "Again" })] };

    expect(() => render(Discover)).not.toThrow();
    await settle();

    expect(screen.getAllByText("Lemon")).toHaveLength(2);
  });
});

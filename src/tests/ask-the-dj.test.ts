import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import Home from "#lib/views/Home.svelte";
import Stats from "#lib/views/Stats.svelte";
import type { AiMix, ListeningStats } from "#lib/api.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeLibrary, mockBackend, resetApp, settle, type Handler } from "./helpers";

// Invented songs only.
function open(handlers: Record<string, Handler> = {}) {
  const lib = makeLibrary(6);
  const backend = mockBackend({
    listening_profile: () => ({ monthArtists: [], monthGenres: [], monthSongs: [], artists: [], genres: [] }),
    ai_recommended: () => ({ basis: "", value: "", at: 0, picks: [] }),
    ...handlers,
  });
  resetApp({ tracks: lib.tracks, albums: [makeAlbum(lib.tracks)] });
  app.ai = { hasKey: true, settings: { provider: "cloud", service: "claude", cloudUrl: "", cloudModel: "", claudeModel: "", webSearch: false } };
  render(Home);
  return { backend, lib, user: userEvent.setup() };
}

async function askFor(user: ReturnType<typeof userEvent.setup>, text: string) {
  await user.click(screen.getByRole("button", { name: /Ask the DJ/ }));
  await user.type(screen.getByRole("textbox", { name: "Ask the DJ" }), text);
  await user.click(screen.getByRole("button", { name: "Ask" }));
  await settle();
}

describe("Ask the DJ: the answer card", () => {
  test("shows what was understood and a relaxed constraint", async () => {
    let ids: string[] = [];
    const answer = (): AiMix => ({
      title: "Rainy Window",
      reason: "Quiet songs for a wet evening.",
      trackIds: ids,
      understood: "Indonesian · calm · not played in 3 months · 4 songs",
      notice: "Your library has only 4 Indonesian songs Cadence can tell, so the list is shorter.",
    });
    const { backend, lib, user } = open({ ai_ask: () => answer() });
    ids = lib.tracks.slice(0, 4).map((t) => t.id);
    await askFor(user, "sad Indonesian songs for a rainy night");
    expect(backend.last("ai_ask")?.request).toBe("sad Indonesian songs for a rainy night");
    expect(screen.getByText("Rainy Window")).toBeInTheDocument();
    expect(screen.getByText("Indonesian · calm · not played in 3 months · 4 songs")).toBeInTheDocument();
    expect(screen.getByText(/only 4 Indonesian songs/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Play 4 songs/ })).toBeInTheDocument();
  });

  test("says when it was picked without the AI", async () => {
    const { lib, user } = open({
      ai_ask: () => ({ title: "Japanese 2010s", reason: "", trackIds: lib.tracks.map((t) => t.id), understood: "Japanese · 2010s · 6 songs", withoutAi: "Picked on this computer without the AI: it didn't answer in time or as expected." }),
    });
    await askFor(user, "Japanese songs from the 2010s");
    expect(screen.getByText(/without the AI/)).toBeInTheDocument();
  });

  test("an older answer without the new fields still renders", async () => {
    const { lib, user } = open({ ai_ask: () => ({ title: "Late Night", reason: "Calm ones.", trackIds: lib.tracks.slice(0, 2).map((t) => t.id) }) });
    await askFor(user, "calm songs");
    expect(screen.getByText("Late Night")).toBeInTheDocument();
    expect(screen.getByText("Calm ones.")).toBeInTheDocument();
    expect(document.querySelector(".understood")).toBeNull();
    expect(document.querySelector(".dj-notice")).toBeNull();
  });
});

describe("Ask the DJ: suggestions", () => {
  test("come from the library's facts", async () => {
    const { backend, user } = open({ dj_suggestions: () => ["Upbeat Japanese songs from the 2010s", "Calm Indonesian songs for a rainy night", "J-Pop I haven't heard in a while"] });
    await user.click(screen.getByRole("button", { name: /Ask the DJ/ }));
    await settle();
    expect(backend.to("dj_suggestions")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Calm Indonesian songs for a rainy night" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Quiet instrumentals" })).toBeNull();
    // Asked once per library, not again on reopening.
    await user.click(screen.getByRole("button", { name: /Ask the DJ/ }));
    await user.click(screen.getByRole("button", { name: /Ask the DJ/ }));
    await settle();
    expect(backend.to("dj_suggestions")).toHaveLength(1);
    // A chip asks.
    await user.click(screen.getByRole("button", { name: "J-Pop I haven't heard in a while" }));
    await settle();
    expect(backend.last("ai_ask")?.request).toBe("J-Pop I haven't heard in a while");
  });

  test("a backend without them keeps the usual ones", async () => {
    const { user } = open({
      dj_suggestions: () => {
        throw new Error("unknown command");
      },
    });
    await user.click(screen.getByRole("button", { name: /Ask the DJ/ }));
    await settle();
    expect(screen.getByRole("button", { name: "Quiet instrumentals" })).toBeInTheDocument();
  });
});

describe("Your Stats: by language", () => {
  const base: ListeningStats = { range: "month", plays: 40, finished: 30, skipped: 5, minutes: 120, songs: 2, artists: 1, albums: 1, topSongs: [], topArtists: [], topAlbums: [], timeline: [] };

  test("shows plays per language, hidden with one language", async () => {
    mockBackend({ listening_stats: () => ({ ...base, languages: [{ code: "ja", name: "Japanese", plays: 30, minutes: 90 }, { code: "id", name: "Indonesian", plays: 10, minutes: 30 }] }) });
    resetApp();
    const { unmount } = render(Stats);
    await settle();
    const s = screen.getByRole("region", { name: "Languages" });
    expect(within(s).getByText("Japanese")).toBeInTheDocument();
    expect(within(s).getByText("Indonesian")).toBeInTheDocument();
    unmount();

    mockBackend({ listening_stats: () => ({ ...base, languages: [{ code: "ja", name: "Japanese", plays: 30, minutes: 90 }] }) });
    resetApp();
    render(Stats);
    await settle();
    expect(screen.queryByRole("region", { name: "Languages" })).toBeNull();
  });
});

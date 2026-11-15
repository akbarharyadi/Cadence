import { render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, test } from "vitest";
import QueuePanel from "#lib/components/QueuePanel.svelte";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

beforeEach(() => mockBackend());

function queueOf(n: number) {
  const tracks = Array.from({ length: n }, () => makeTrack());
  resetApp({ tracks, albums: [makeAlbum(tracks)], status: makeStatus({ queue: tracks.map((t) => t.id), index: 0, duration: 200, playing: true }) });
}

describe("QueuePanel", () => {
  test("a 500-song queue says 499 songs are coming up, not 200", async () => {
    queueOf(500);
    render(QueuePanel);
    await settle();

    expect(screen.getByText(/Next, 499 songs/)).toBeInTheDocument();
    expect(screen.queryByText(/Next, 200 songs/)).not.toBeInTheDocument();
    // It lists only the first 200 and says so.
    expect(screen.getByText(/first 200 shown/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Remove from queue" })).toHaveLength(200);
  });

  test("a short queue says its size and does not mention a limit", async () => {
    queueOf(4);
    render(QueuePanel);
    await settle();
    expect(screen.getByText("Next, 3 songs")).toBeInTheDocument();
    expect(screen.queryByText(/shown/)).not.toBeInTheDocument();
  });

  test("the count follows the queue when it grows", async () => {
    queueOf(300);
    render(QueuePanel);
    await settle();
    expect(screen.getByText(/Next, 299 songs/)).toBeInTheDocument();

    const more = Array.from({ length: 700 }, () => makeTrack());
    app.tracks = [...app.tracks, ...more];
    app.status = makeStatus({ ...app.status!, queue: [...app.status!.queue, ...more.map((t) => t.id)] });
    await settle();
    expect(screen.getByText(/Next, 999 songs/)).toBeInTheDocument();
  });
});

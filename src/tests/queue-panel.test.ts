import { fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import QueuePanel from "#lib/components/QueuePanel.svelte";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

let backend: ReturnType<typeof mockBackend>;
beforeEach(() => (backend = mockBackend()));
afterEach(() => document.documentElement.classList.remove("phone"));

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

  test("on a phone a tap plays the song, but a tap on its remove button does not", async () => {
    document.documentElement.classList.add("phone");
    queueOf(3);
    render(QueuePanel);
    await settle();
    const removes = screen.getAllByRole("button", { name: "Remove from queue" });
    await fireEvent.click(removes[0]);
    expect(backend.to("jump")).toHaveLength(0);
    await fireEvent.click(removes[1].closest(".item")!);
    expect(backend.last("jump")).toEqual({ index: 2 });
  });

  test("on a desktop a single click does not play the song", async () => {
    queueOf(3);
    render(QueuePanel);
    await settle();
    await fireEvent.click(screen.getAllByRole("button", { name: "Remove from queue" })[0].closest(".item")!);
    expect(backend.to("jump")).toHaveLength(0);
  });

  test("Back closes Up Next before Now Playing (it opens over it on phones)", () => {
    app.nowPlayingOpen = true;
    app.queueOpen = true;
    app.handleBack();
    expect(app.queueOpen).toBe(false);
    expect(app.nowPlayingOpen).toBe(true);
    app.nowPlayingOpen = false;
  });
});

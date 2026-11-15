import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import LyricsView from "#lib/components/LyricsView.svelte";
import type { Lyrics } from "#lib/api.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

// Lyrics made for another version of a song can run early or late: the listener nudges them, and the
// nudge is kept per song (localStorage "cadence-lyric-offsets"). A positive offset means the lyrics
// come later than their timestamps say, so the line stamped 30 s is sung at 30 + offset.
// One word per line: the line being sung is drawn word by word (for the sweep), which would split a longer name.
const lyrics: Lyrics = {
  source: "embedded",
  plain: "",
  instrumental: false,
  lookup: "",
  synced: [
    { time: 10, text: "Alpha" },
    { time: 20, text: "Bravo" },
    { time: 30, text: "Charlie" },
    { time: 40, text: "Delta" },
  ],
};
const track = makeTrack({ title: "Lemon", duration: 120 });

async function open(offset?: number) {
  if (offset !== undefined) localStorage.setItem("cadence-lyric-offsets", JSON.stringify({ [track.id]: offset }));
  const backend = mockBackend({ get_lyrics: () => lyrics });
  resetApp({ tracks: [track], albums: [makeAlbum([track])], status: makeStatus({ queue: [track.id], index: 0, duration: 120, playing: false }) });
  render(LyricsView, { trackId: track.id });
  await settle();
  return { backend, user: userEvent.setup() };
}
const line = (text: string) => screen.getByRole("button", { name: text });
const seekedTo = (b: ReturnType<typeof mockBackend>) => b.last("seek")?.position as number;

describe("LyricsView: clicking a line with a stored timing offset", () => {
  test.each([2.5, -1.5])("offset %s: seeks so the clicked line is the one being sung", async (offset) => {
    const { backend, user } = await open(offset);

    await user.click(line("Charlie"));

    expect(seekedTo(backend)).toBeCloseTo(30 + offset, 5);
    // After the seek the lyrics see the position minus the offset: that is the Charlie's own time.
    expect(line("Charlie")).toHaveClass("active");
    expect(line("Bravo")).not.toHaveClass("active");
    expect(line("Delta")).not.toHaveClass("active");
  });

  test("no stored offset: seeks to the line's own time", async () => {
    const { backend, user } = await open();
    await user.click(line("Bravo"));
    expect(seekedTo(backend)).toBe(20);
    expect(line("Bravo")).toHaveClass("active");
  });

  test("an offset only applies to its own song", async () => {
    localStorage.setItem("cadence-lyric-offsets", JSON.stringify({ "some-other-song": 4 }));
    const { backend, user } = await open();
    await user.click(line("Charlie"));
    expect(seekedTo(backend)).toBe(30);
  });

  test("a line near the start never seeks before 0", async () => {
    const { backend, user } = await open(-15);
    await user.click(line("Alpha")); // 10 - 15 = -5
    expect(seekedTo(backend)).toBe(0);
  });
});

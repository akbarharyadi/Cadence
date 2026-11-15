import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, test } from "vitest";
import LyricsView from "#lib/components/LyricsView.svelte";
import type { Lyrics, LyricsTranslation, RubyPart } from "#lib/api.js";
import { rubyPieces, checkedRuby } from "#lib/ruby.js";
import { app } from "#lib/store.svelte.js";
import { makeAlbum, makeStatus, makeTrack, mockBackend, resetApp, settle } from "./helpers";

// Invented lines (no real lyrics in fixtures).
const lyrics: Lyrics = {
  source: "lrclib",
  plain: "",
  instrumental: false,
  lookup: "",
  synced: [
    { time: 1, text: "今日は雨" },
    { time: 10, text: "青い空" },
  ],
};
const ruby: RubyPart[][] = [
  [{ text: "今日", ruby: "きょう" }, { text: "は" }, { text: "雨", ruby: "あめ" }],
  [{ text: "青", ruby: "あお" }, { text: "い" }, { text: "空", ruby: "そら" }],
];
const first = makeTrack({ title: "Test Song", artist: "Example Artist", duration: 120 });
const second = makeTrack({ title: "Other Song", artist: "Example Artist", duration: 120 });

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

function open(handlers: Record<string, (args: Record<string, unknown>) => unknown> = {}) {
  const backend = mockBackend({ get_lyrics: () => lyrics, ruby_lyrics: () => ruby, romanize_lyrics: () => ["", ""], ...handlers });
  resetApp({ tracks: [first, second], albums: [makeAlbum([first, second])], status: makeStatus({ queue: [first.id], index: 0, duration: 120 }) });
  app.position = 2; // the first line is being sung
  return backend;
}
const chip = () => screen.getByRole("button", { name: /(Translation|Translating…)$/ });

beforeEach(() => localStorage.clear());

describe("LyricsView: translation under each line", () => {
  test("off: nothing is asked and nothing shows", async () => {
    const backend = open({ translate_lyrics: () => ({ lang: "en", from: "ai", lines: ["a", "b"] }) });
    render(LyricsView, { trackId: first.id });
    await settle();
    expect(chip()).toHaveAttribute("aria-pressed", "false");
    expect(backend.to("translate_lyrics")).toHaveLength(0);
    expect(document.querySelector(".tr")).toBeNull();
  });

  test("turned on: translating, then each line's translation under it", async () => {
    const answer = deferred<LyricsTranslation>();
    const backend = open({ translate_lyrics: () => answer.promise });
    render(LyricsView, { trackId: first.id });
    await settle();

    await fireEvent.click(chip());
    await settle();
    expect(chip()).toHaveTextContent("Translating…");
    expect(backend.last("translate_lyrics")).toEqual({ trackId: first.id, lang: "en" });

    answer.resolve({ lang: "en", from: "ai", lines: ["it is raining today", "the blue sky"] });
    await waitFor(() => expect(document.querySelectorAll(".tr")).toHaveLength(2));
    expect([...document.querySelectorAll(".tr")].map((e) => e.textContent)).toEqual(["it is raining today", "the blue sky"]);
    expect(chip()).toHaveTextContent("Translation");
    expect(chip()).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem("cadence-translate")).toBe("on");
  });

  test("a translation that doesn't line up with the lines isn't shown", async () => {
    localStorage.setItem("cadence-translate", "on");
    open({ translate_lyrics: () => ({ lang: "en", from: "ai", lines: ["only one"] }) });
    render(LyricsView, { trackId: first.id });
    await waitFor(() => expect(screen.getByText("Couldn’t translate")).toBeInTheDocument());
    expect(document.querySelector(".tr")).toBeNull();
  });

  test("AI not set up: says so and links to Settings", async () => {
    localStorage.setItem("cadence-translate", "on");
    open({ translate_lyrics: () => ({ lang: "en", from: "needs-ai", lines: [] }) });
    app.nowPlayingOpen = true;
    render(LyricsView, { trackId: first.id });
    const link = await screen.findByRole("button", { name: /Needs the AI DJ/ });
    expect(document.querySelector(".tr")).toBeNull();
    await fireEvent.click(link);
    expect(app.view).toEqual({ name: "settings" });
    expect(app.nowPlayingOpen).toBe(false);
  });

  test("the chosen language is asked for and kept", async () => {
    localStorage.setItem("cadence-translate", "on");
    const backend = open({ translate_lyrics: (a) => ({ lang: a.lang, from: "ai", lines: ["x", "y"] }) });
    render(LyricsView, { trackId: first.id });
    await settle();
    await fireEvent.change(screen.getByRole("combobox", { name: "Translate into" }), { target: { value: "id" } });
    await waitFor(() => expect(backend.last("translate_lyrics")).toEqual({ trackId: first.id, lang: "id" }));
    expect(localStorage.getItem("cadence-translate-lang")).toBe("id");
  });

  test("new timings for the same lines (the singer followed) keep the translation: no new request", async () => {
    localStorage.setItem("cadence-translate", "on");
    let served: Lyrics = lyrics;
    const backend = open({ get_lyrics: () => served, translate_lyrics: () => ({ lang: "en", from: "ai", lines: ["it is raining today", "the blue sky"] }) });
    render(LyricsView, { trackId: first.id });
    await waitFor(() => expect(document.querySelectorAll(".tr")).toHaveLength(2));
    expect(backend.to("translate_lyrics")).toHaveLength(1);

    // A voice job finished: the same lines, placed on the singing (other times).
    served = { ...lyrics, synced: [{ time: 1.4, text: "今日は雨" }, { time: 10.8, text: "青い空" }] };
    app.voiceDone = { trackId: first.id, error: "", at: Date.now() };
    await settle();
    await settle();
    expect(backend.to("get_lyrics").length).toBeGreaterThanOrEqual(2);
    expect(backend.to("translate_lyrics")).toHaveLength(1);
    expect(backend.to("translate_cancel")).toHaveLength(0);
    expect(document.querySelectorAll(".tr")).toHaveLength(2);

    // Other lines (another version chosen): translated anew.
    served = { ...lyrics, synced: [{ time: 1, text: "今日は晴れ" }, { time: 10, text: "青い空" }] };
    app.voiceDone = { trackId: first.id, error: "", at: Date.now() + 1 };
    await waitFor(() => expect(backend.to("translate_lyrics")).toHaveLength(2));
  });

  test("another song cancels the one being translated", async () => {
    localStorage.setItem("cadence-translate", "on");
    const backend = open({ translate_lyrics: () => new Promise(() => {}) });
    const view = render(LyricsView, { trackId: first.id });
    await settle();
    await view.rerender({ trackId: second.id });
    await settle();
    expect(backend.to("translate_cancel").map((c) => c.args.trackId)).toContain(first.id);
    expect(backend.last("translate_lyrics")).toEqual({ trackId: second.id, lang: "en" });
  });
});

describe("LyricsView: furigana", () => {
  test("readings above the kanji on every line, inside the sung line's sweep too", async () => {
    localStorage.setItem("cadence-furigana", "on");
    open();
    render(LyricsView, { trackId: first.id });
    await waitFor(() => expect(document.querySelectorAll("rt").length).toBeGreaterThanOrEqual(4));
    const [sung, next] = [...document.querySelectorAll<HTMLElement>(".line")];
    expect(sung).toHaveClass("active");
    // The sung line: 今 and 日 are swept as one piece, so its reading isn't split.
    const pieces = [...sung.querySelectorAll<HTMLElement>(".w")];
    expect(pieces[0].querySelector("ruby")?.textContent).toBe("今日きょう");
    expect(pieces.map((p) => p.querySelector("rt")?.textContent ?? "")).toEqual(["きょう", "", "あめ"]);
    // The next line has its readings too (the room for them doesn't appear when it's sung).
    expect([...next.querySelectorAll("rt")].map((r) => r.textContent)).toEqual(["あお", "そら"]);
    expect(document.querySelector(".lyrics")).toHaveClass("furi");
    expect(screen.getByRole("button", { name: /Furigana$/ })).toHaveAttribute("aria-pressed", "true");
  });

  test("readings that don't spell the line are left out", async () => {
    localStorage.setItem("cadence-furigana", "on");
    open({ ruby_lyrics: () => [[{ text: "違う", ruby: "ちがう" }], "not a list"] });
    render(LyricsView, { trackId: first.id });
    await settle();
    await settle();
    expect(document.querySelectorAll("rt")).toHaveLength(0);
    expect(screen.getByRole("button", { name: /今日は雨/ })).toBeInTheDocument();
  });
});

describe("ruby pieces", () => {
  const piece = (text: string, start: number) => ({ text, start, end: start + 1 });
  test("pieces a reading covers are swept as one; others stay", () => {
    const out = rubyPieces([piece("今", 0), piece("日", 1), piece("は", 2), piece("雨", 3)], ruby[0]);
    expect(out.map((p) => [p.text, p.start, p.end])).toEqual([["今日", 0, 2], ["は", 2, 3], ["雨", 3, 4]]);
    expect(out[0].parts).toEqual([{ text: "今日", ruby: "きょう" }]);
  });
  test("word pieces with a space, and words that leave out the start of the line", () => {
    const parts: RubyPart[] = [{ text: "Test " }, { text: "雨", ruby: "あめ" }, { text: "が降る" }];
    const out = rubyPieces([piece("雨が", 0), piece("降る ", 1)], parts);
    expect(out[0].parts).toEqual([{ text: "雨", ruby: "あめ" }, { text: "が" }]);
    expect(out[1].parts).toEqual([{ text: "降る" }, { text: " " }]);
    // Nothing in common: plain pieces.
    expect(rubyPieces([piece("abc", 0)], parts)[0].parts).toEqual([{ text: "abc" }]);
  });
  test("checked against the lines", () => {
    expect(checkedRuby(["今日は雨", "x"], [ruby[0], []])).toEqual([ruby[0], undefined]);
    expect(checkedRuby(["今日は雨"], null)).toEqual([undefined]);
  });
});

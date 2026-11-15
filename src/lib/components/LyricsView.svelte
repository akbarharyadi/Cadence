<script lang="ts">
  import { api, formatTime, type LyricLine, type Lyrics, type LyricsHit, type LyricsTranslation, type RubyPart, type TranslateLang } from "#lib/api.js";
  import { checkedRuby, rubyPieces } from "#lib/ruby.js";
  import { app } from "#lib/store.svelte.js";
  import { untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import Icon from "./Icon.svelte";

  let { trackId }: { trackId: string } = $props();

  const track = $derived(app.trackById.get(trackId));
  let lyrics = $state<Lyrics | null>(null);
  let loading = $state(true);
  let box = $state<HTMLDivElement>();
  let userScrolledAt = 0;

  // `trackId` is read through the current song object, which is replaced on every library update;
  // a derived only changes when the id really does, so lyrics aren't reloaded (and the view doesn't
  // jump to the top) when the library is rescanned.
  const songId = $derived(trackId);

  $effect(() => {
    const id = songId;
    loading = true;
    lyrics = null;
    panel = null;
    api
      .lyrics(id)
      .then((l) => {
        if (id === songId) lyrics = l;
      })
      // No lyrics (offline, or the lookup failed): the view's empty state, quietly.
      .catch(() => {})
      .finally(() => {
        if (id === songId) loading = false;
      });
  });

  // Pronunciation: the lyrics in Latin letters under each line, for scripts you can't read.
  const allText = $derived(lyrics ? lyrics.synced.map((l) => l.text).join("\n") + "\n" + (lyrics.plain ?? "") : "");
  /** What the Latin letters are called for this song's script, or "" if it needs none. */
  const romanName = $derived(
    /[\u3040-\u30ff]/.test(allText) ? "Romaji" : /[\uac00-\ud7a3]/.test(allText) ? "Romanization" : /[\u3400-\u9fff]/.test(allText) ? "Pinyin" : "",
  );
  let pronounce = $state(typeof localStorage !== "undefined" && localStorage.getItem("cadence-pronounce") === "on");
  let roman = $state<string[]>([]);
  const plainLines = $derived(lyrics?.plain ? lyrics.plain.split("\n") : []);
  $effect(() => {
    const l = lyrics;
    roman = [];
    if (!pronounce || !romanName || !l) return;
    const lines = l.synced.length ? l.synced.map((x) => x.text) : (l.plain ?? "").split("\n");
    api
      .romanize(lines)
      .then((r) => {
        if (lyrics === l) roman = r;
      })
      .catch(() => {});
  });
  function togglePronounce() {
    pronounce = !pronounce;
    localStorage.setItem("cadence-pronounce", pronounce ? "on" : "off");
  }
  const stored = (key: string) => (typeof localStorage !== "undefined" ? localStorage.getItem(key) : null);
  /** The lines as shown (timed ones, or the plain text's), which readings and translations follow. */
  const shownLines = $derived(lyrics ? (lyrics.synced.length ? lyrics.synced.map((x) => x.text) : plainLines) : []);

  // ---------- readings above the characters (furigana over kanji, pinyin over hanzi) ----------

  const rubyName = $derived(romanName === "Romaji" ? (/[㐀-鿿]/.test(allText) ? "Furigana" : "") : romanName === "Pinyin" ? "Pinyin above" : "");
  let furigana = $state(stored("cadence-furigana") === "on");
  const rubyOn = $derived(furigana && !!rubyName);
  let rubyRaw = $state<RubyPart[][] | null>(null);
  $effect(() => {
    const l = lyrics;
    rubyRaw = null;
    if (!rubyOn || !l) return;
    api
      .rubyLyrics(untrack(() => shownLines))
      .then((r) => {
        if (lyrics === l) rubyRaw = r;
      })
      .catch(() => {});
  });
  /** Per line its pieces with readings, only where they spell the line exactly. */
  const rubyLines = $derived(rubyOn ? checkedRuby(shownLines, rubyRaw) : []);
  function toggleFurigana() {
    furigana = !furigana;
    localStorage.setItem("cadence-furigana", furigana ? "on" : "off");
  }

  // ---------- translation under each line ----------

  const LANG_NAMES: Record<TranslateLang, string> = { en: "English", id: "Indonesian", ja: "Japanese", ko: "Korean", zh: "Chinese" };
  const isLang = (s: string | null): s is TranslateLang => !!s && s in LANG_NAMES;
  /** The window's language, when it's one of those; else English. */
  function systemLang(): TranslateLang {
    const n = (typeof navigator !== "undefined" ? navigator.language : "en").toLowerCase();
    if (n.startsWith("id") || n.startsWith("in")) return "id";
    for (const c of ["ja", "ko", "zh"] as const) if (n.startsWith(c)) return c;
    return "en";
  }
  let translateOn = $state(stored("cadence-translate") === "on");
  const savedLang = stored("cadence-translate-lang");
  let translateLang = $state<TranslateLang>(isLang(savedLang) ? savedLang : systemLang());
  let translation = $state<LyricsTranslation | null>(null);
  let translateState = $state<"off" | "translating" | "shown" | "needs-ai" | "same" | "error">("off");
  let translateError = $state("");
  const hasText = $derived(!!lyrics && !lyrics.instrumental && (lyrics.synced.length > 0 || !!lyrics.plain?.trim()));
  /**
   * What a translation is for: the song, the language and the lines' text. The lyrics object is
   * replaced when only timings or singers change (a voice job finishing, "Find singers"): the
   * same text then keeps the translation shown, with no new request (and no tokens spent again).
   */
  const translateFor = $derived(translateOn && lyrics && hasText ? `${songId}\u0000${translateLang}\u0000${shownLines.length}\u0000${shownLines.join("\n")}` : "");
  // Asked only while the toggle is on, for the song on screen; another song (or other lines) cancels it.
  $effect(() => {
    const key = translateFor;
    translation = null;
    translateError = "";
    if (!key) {
      translateState = "off";
      return;
    }
    const id = untrack(() => songId);
    const lang = untrack(() => translateLang);
    translateState = "translating";
    const count = untrack(() => shownLines.length);
    let current = true;
    api
      .translateLyrics(id, lang)
      .then((t) => {
        if (!current) return;
        if (!t || !Array.isArray(t.lines)) throw new Error("No translation came back.");
        if (t.from === "needs-ai") translateState = "needs-ai";
        else if (t.from === "same" || t.from === "none") translateState = "same";
        else if (t.lines.length !== count) throw new Error("The translation didn’t line up with the lyrics.");
        else {
          translation = t;
          translateState = "shown";
        }
      })
      .catch((e) => {
        if (!current) return;
        translateState = "error";
        translateError = e instanceof Error ? e.message : String(e);
      });
    return () => {
      current = false;
      api.translateCancel(id)?.catch?.(() => {});
    };
  });
  const translatedLines = $derived(translateState === "shown" && translation ? translation.lines.map((x) => (typeof x === "string" ? x : "")) : []);
  function toggleTranslate() {
    translateOn = !translateOn;
    localStorage.setItem("cadence-translate", translateOn ? "on" : "off");
  }
  function setTranslateLang(lang: string) {
    if (!isLang(lang)) return;
    translateLang = lang;
    localStorage.setItem("cadence-translate-lang", lang);
  }
  function openAiSettings() {
    app.nowPlayingOpen = false;
    app.go({ name: "settings" });
  }

  // Timing nudge for this song (lyrics made for another version can run early or late), kept
  // per song on this device. Positive: the lyrics come later.
  /** Following the singer runs on the graphics card on Windows (DirectML), on the processor elsewhere. */
  const voiceSpeed =
    typeof navigator !== "undefined" && navigator.userAgent.includes("Windows")
      ? "well under a minute per song with a graphics card, 2–3 minutes without"
      : "about 2–3 minutes per song";
  const OFFSETS = "cadence-lyric-offsets";
  const readOffsets = (): Record<string, number> => {
    try {
      return JSON.parse(localStorage.getItem(OFFSETS) ?? "{}");
    } catch {
      return {};
    }
  };
  let offset = $state(0);
  /** Lyrics placed on the singing need no nudge of their own: they keep a separate one. */
  const offsetKey = $derived(lyrics?.voice === "on" ? `${songId}:voice` : songId);
  $effect(() => {
    offset = readOffsets()[offsetKey] ?? 0;
  });
  /** The timing nudge shows when asked for (⋯ → Adjust timing) or while this song has one. */
  let timingOpen = $state(false);
  const reducedMotion = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  function nudge(by: number) {
    offset = by === 0 ? 0 : Math.round((offset + by) * 100) / 100;
    const all = readOffsets();
    if (offset) all[offsetKey] = offset;
    else delete all[offsetKey];
    localStorage.setItem(OFFSETS, JSON.stringify(all));
  }
  /**
   * Where the song is, as the lyrics see it (30 times a second, for the sweep). The timer runs only
   * while timed lyrics are on screen, the song is playing and the window is visible; otherwise the
   * position follows the player's own four updates a second.
   */
  let live = $state(app.position);
  // The page can be seen (the window not hidden in the tray or minimized; see showing.svelte.ts).
  const visible = $derived(app.showing);
  const playing = $derived(!!app.status?.playing);
  $effect(() => {
    if (!(lyrics?.synced.length && panel === null && playing && visible)) {
      live = app.position;
      return;
    }
    live = untrack(() => app.exactPosition()); // untracked: it reads the status, which is replaced four times a second
    const timer = setInterval(() => (live = app.exactPosition()), 33);
    return () => clearInterval(timer);
  });
  const pos = $derived(live - offset);

  /** Index of the line being sung (slightly early, so the eye gets there in time). */
  const active = $derived.by(() => {
    const lines = lyrics?.synced ?? [];
    const t = pos + 0.25;
    let i = -1;
    for (let k = 0; k < lines.length && lines[k].time <= t; k++) i = k;
    return i;
  });

  // The user is reading ahead: wheel, touch, keyboard, or any other scroll that isn't our own.
  // Our own scrolls (below) are marked for as long as they can last, so their `scroll` events
  // aren't taken for the user's.
  let ownScrollUntil = 0;
  // Where `scrollend` exists the smooth scroll says itself when it is over (a long one can outlast
  // any fixed window); elsewhere the window is a guess.
  const hasScrollEnd = typeof window !== "undefined" && "onscrollend" in window;
  function onScrollEnd() {
    ownScrollUntil = Date.now() + 100;
  }
  const markUser = () => (userScrolledAt = Date.now());
  function onScroll() {
    if (Date.now() > ownScrollUntil) markUser();
  }
  function onKey(e: KeyboardEvent) {
    if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key)) markUser();
  }
  function centre(el: HTMLElement | undefined, smooth: boolean) {
    if (!el) return;
    ownScrollUntil = Date.now() + (smooth ? (hasScrollEnd ? 5000 : 1200) : 150);
    el.scrollIntoView({ behavior: smooth ? "smooth" : "instant", block: "center" });
  }

  // Keep the current line in the middle, unless the user has just scrolled to read ahead.
  $effect(() => {
    const i = active;
    void roman; // pronunciation lines change the layout: centre again
    void translatedLines; // so do translations and readings above the lines
    void rubyLines;
    if (i < 0 || !box || Date.now() - userScrolledAt < 4000) return;
    centre(box.querySelectorAll<HTMLElement>(".line")[i], !reducedMotion());
  });

  // The window (or the lyrics' column) changed size: the lines wrap differently, so the current
  // one goes back to the middle at once.
  $effect(() => {
    const el = box;
    if (!el) return;
    let last = el.clientWidth + el.clientHeight * 10000;
    const ro = new ResizeObserver(() => {
      const now = el.clientWidth + el.clientHeight * 10000;
      if (now === last) return;
      last = now;
      if (Date.now() - userScrolledAt < 4000) return;
      centre(el.querySelectorAll<HTMLElement>(".line")[untrack(() => active)], false);
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  // ---------- karaoke sweep ----------

  interface Piece {
    text: string;
    start: number;
    end: number;
  }

  /**
   * The words of a line with their timing. Word-timed lyrics use their own timings; otherwise the
   * line's time is shared out by length (per character for text without spaces, like Japanese).
   * Singers usually carry a line until just before the next one, so the sweep fills that time.
   * `quick` is how long the line takes sung quickly (a third of a second per Japanese, Chinese or
   * Korean character, much less per Latin letter); real lines take about 1.8 times that, and slow
   * ones (a held chorus) up to 5 times. Only a gap longer than even that means an instrumental
   * break follows, and the sweep then takes the usual singing time.
   */
  function pieces(line: LyricLine, next: LyricLine | undefined): Piece[] {
    if (line.words?.length) {
      return line.words.map((w, i) => ({
        text: w.text,
        start: w.time,
        end: w.end ?? line.words![i + 1]?.time ?? Math.min(next?.time ?? w.time + 1, w.time + 1.5),
      }));
    }
    const parts = /\s/.test(line.text) ? splitAfterSpace(line.text) : [...line.text];
    const chars = parts.reduce((n, p) => n + p.trim().length, 0) || 1;
    const cjk = (line.text.match(/[\u3040-\u30ff\u3400-\u9fff\uac00-\ud7a3]/g) ?? []).length;
    const quick = Math.max(1.2, cjk * 0.33 + (chars - cjk) * 0.07);
    const gap = next ? next.time - line.time : Infinity;
    const span = Math.max(0.3, gap <= quick * 5 + 1 ? gap - 0.25 : quick * 1.8);
    let t = line.time;
    return parts.map((p) => {
      const d = (span * Math.max(p.trim().length, 0.5)) / chars;
      const piece = { text: p, start: t, end: t + d };
      t += d;
      return piece;
    });
  }

  const sweep = $derived(active >= 0 && lyrics ? pieces(lyrics.synced[active], lyrics.synced[active + 1]) : []);
  /** The sung line's pieces with their readings above (worked out once per line, not per tick). */
  const sweepRuby = $derived(rubyPieces(sweep, active >= 0 ? rubyLines[active] : undefined));
  const progress = (p: Piece) => Math.min(1, Math.max(0, (pos + 0.1 - p.start) / (p.end - p.start)));

  /** How much of the sung line has been swept (0 to 1), weighted by each piece's length. */
  const lineFill = $derived.by(() => {
    let total = 0;
    let done = 0;
    for (const p of sweep) {
      const n = p.text.trim().length || 0.5;
      total += n;
      done += n * progress(p);
    }
    return total ? done / total : 0;
  });

  /** Splits after every whitespace character ("a b" -> "a ", "b"); no lookbehind, which older WebKit can't parse. */
  function splitAfterSpace(text: string): string[] {
    return text.match(/[^\s]*\s|[^\s]+$/g) ?? [text];
  }

  /** The pronunciation line in words, each with its share of the line, to sweep along with it. */
  function romanWords(text: string) {
    const words = splitAfterSpace(text);
    const total = words.reduce((n, w) => n + w.trim().length, 0) || 1;
    let at = 0;
    return words.map((w) => {
      const from = at / total;
      at += w.trim().length;
      return { text: w, from, to: at / total };
    });
  }
  const wordFill = (w: { from: number; to: number }) =>
    Math.min(1, Math.max(0, (lineFill - w.from) / Math.max(w.to - w.from, 0.001))) * 100;

  // ---------- find / fix lyrics ----------

  let panel = $state<"search" | "paste" | "voice" | null>(null);
  // What's searched for, prefilled from the song's tags (editable: tags can be wrong).
  let query = $state({ title: "", artist: "", album: "" });
  // The song the search fields and results are for (they're filled again for another song).
  let searchedFor = "";
  let hits = $state<LyricsHit[] | null>(null);
  let busy = $state<"" | "search" | "ai" | "use">("");
  let error = $state("");
  let pasted = $state("");

  function openSearch() {
    panel = "search";
    error = "";
    if (searchedFor !== trackId && track) {
      searchedFor = trackId;
      hits = null;
      const title = track.title.split(/[(\[（【]/)[0].trim();
      const latin = title.replace(/[^\x00-\x7F]/g, "").trim();
      query = {
        title: latin.replace(/[^A-Za-z0-9]/g, "").length >= 2 ? latin : title,
        artist: track.artist ?? "",
        album: track.album ?? "",
      };
    }
    if (!hits) runSearch();
  }

  async function runSearch() {
    const { title, artist, album } = query;
    if (!title.trim() && !artist.trim()) return;
    const id = trackId;
    busy = "search";
    error = "";
    try {
      const found = await api.searchLyrics(id, title, artist, album);
      if (id === trackId) hits = found;
    } catch (e) {
      if (id === trackId) error = String(e);
    } finally {
      busy = "";
    }
  }

  async function choose(h: LyricsHit) {
    busy = "use";
    try {
      lyrics = await api.useLyrics(trackId, h.id);
      panel = null;
    } catch (e) {
      error = String(e);
    } finally {
      busy = "";
    }
  }

  async function findWithAi() {
    busy = "ai";
    error = "";
    try {
      lyrics = await api.aiFindLyrics(trackId);
      panel = null;
    } catch (e) {
      error = String(e);
    } finally {
      busy = "";
    }
  }

  async function hideLyrics() {
    await api.hideLyrics(trackId);
    lyrics = { source: "", synced: [], plain: "", instrumental: false, lookup: "hidden" };
  }

  async function savePasted() {
    try {
      lyrics = await api.pasteLyrics(trackId, pasted);
      panel = null;
      pasted = "";
    } catch (e) {
      error = String(e);
    }
  }

  // ---------- follow the singer ----------

  /** Lyrics that can be placed on the singing (timed or not). */
  const placeable = $derived(!!lyrics && !lyrics.instrumental && (lyrics.synced.length > 0 || !!lyrics.plain?.trim()));
  /** Songs this view has asked Cadence to listen to (once each, even if it failed). */
  const asked = new SvelteSet<string>();
  let voiceError = $state("");
  /** Following failed because most lines aren't in the singing (voice/mod.rs: "Couldn’t find these
   *  lyrics in the singing"): other lyrics are the fix, not another try. */
  const lyricsMismatch = $derived(voiceError.includes("find these lyrics"));
  const listening = $derived(app.voice?.job?.trackId === songId ? app.voice.job : null);
  const waiting = $derived(asked.has(songId) && !listening && !lyrics?.voice && !voiceError && !!app.voice?.job);

  function follow() {
    if (!app.voice?.installed) {
      panel = "voice";
      return;
    }
    voiceError = "";
    asked.add(songId);
    api.voiceFollow(songId);
  }

  // Automatically, once the models are there: every song whose lyrics are opened.
  $effect(() => {
    const id = songId;
    // Lyrics that came timed word by word (LRCLIB, enhanced LRC) follow the singing already.
    const sourceWords = !!lyrics?.synced.some((l) => l.words?.length);
    if (!placeable || lyrics?.voice || sourceWords || !app.voice?.installed || !app.voice.auto || untrack(() => asked.has(id))) return;
    asked.add(id);
    voiceError = "";
    api.voiceFollow(id).catch(() => {});
  });

  // Done listening to this song: show the placed lyrics (or why not).
  $effect(() => {
    const done = app.voiceDone;
    if (!done || done.trackId !== songId) return;
    if (done.error) {
      if (asked.has(songId)) voiceError = done.error;
      return;
    }
    const id = songId;
    api
      .lyrics(id)
      .then((l) => {
        if (id === songId) lyrics = l;
      })
      .catch(() => {});
  });

  // The models just finished downloading from this view's panel: start on this song.
  $effect(() => {
    if (panel === "voice" && app.voice?.installed && !app.voice.installing) {
      panel = null;
      follow();
    }
  });

  async function toggleVoice() {
    if (!lyrics?.voice) return;
    const id = songId;
    const l = await api.voiceUse(id, lyrics.voice !== "on");
    if (id === songId) lyrics = l;
  }

  async function downloadVoice() {
    await api.voiceInstall();
    app.voice = await api.voiceStatus();
  }

  // ---------- singers ----------

  /** Each singer's colour (r, g, b): the first stays white; lines sung together are lavender. */
  const SINGER_COLORS = ["255, 255, 255", "255, 158, 207", "130, 210, 255", "255, 210, 120", "165, 235, 165", "255, 170, 140", "200, 170, 255"];
  const TOGETHER = "205, 185, 255";
  let showSingers = $state(typeof localStorage === "undefined" || localStorage.getItem("cadence-singers") !== "off");
  const hasSingers = $derived(!!lyrics?.singers?.length && !!lyrics.synced.some((l) => l.singers?.length));
  const singersOn = $derived(showSingers && hasSingers);
  function singerName(i: number) {
    const name = lyrics?.singers?.[i];
    if (name) return name;
    if (lyrics?.singersFrom === "voice") return i === 0 ? "Lower voice" : "Higher voice";
    return `Singer ${i + 1}`;
  }
  function singerColor(line: LyricLine) {
    const s = line.singers ?? [];
    if (!singersOn || !s.length) return SINGER_COLORS[0];
    return s.length > 1 ? TOGETHER : SINGER_COLORS[s[0] % SINGER_COLORS.length];
  }
  /**
   * Lines not being sung keep (almost) the usual white, so they stay readable on any cover's
   * colours: only a quarter of the singer's colour. The full colour is for the name and the line
   * being sung.
   */
  function tint(rgb: string) {
    return rgb
      .split(",")
      .map((c) => Math.round(255 * 0.75 + +c * 0.25))
      .join(", ");
  }
  /** The name to show above a line: only where the singer changes. */
  const singerLabels = $derived.by(() => {
    const out: string[] = [];
    let last = "";
    for (const line of lyrics?.synced ?? []) {
      const s = line.singers ?? [];
      const key = s.join(",");
      if (!singersOn || !line.text || !s.length || key === last) {
        out.push("");
        if (line.text && s.length) last = key;
        continue;
      }
      last = key;
      const all = s.length > 1 && s.length === (lyrics?.singers?.length ?? 0);
      const named = s.every((k) => !!lyrics?.singers?.[k]);
      // Everyone together: "Both" / "Everyone" unless their names are short enough to read.
      out.push(all && (!named || s.length > 2) ? (s.length > 2 ? "Everyone" : "Both") : s.map(singerName).join(" & "));
    }
    return out;
  });
  function toggleSingers() {
    showSingers = !showSingers;
    localStorage.setItem("cadence-singers", showSingers ? "on" : "off");
  }
  let findingSingers = $state(false);
  async function findSingers() {
    if (findingSingers) return;
    findingSingers = true;
    if (app.aiReady) app.showToast("The AI is searching the web for the singers; this can take a minute or two.");
    const id = songId;
    try {
      const l = await api.findSingers(id);
      if (id !== songId) return;
      lyrics = l;
      showSingers = true;
      localStorage.setItem("cadence-singers", "on");
      if (l.singers?.length) app.showToast(`Singers: ${l.singers.map((_, i) => singerName(i)).join(", ")}`);
      else if (app.voice?.job) app.showToast("Listening to the voices; the singers show when that’s done.");
      else
        app.showToast(
          app.aiReady
            ? "No second credited singer was found for this song."
            : "Cadence tells singers apart only in songs credited to several singers.",
        );
    } catch (e) {
      app.showToast(String(e));
    } finally {
      findingSingers = false;
    }
  }

  function moreMenu(e: MouseEvent) {
    const timed = !!lyrics?.synced.length;
    const canFind = timed && lyrics?.singersFrom !== "lyrics" && (app.aiReady || !!app.voice?.installed);
    // Following the singer: start it, or switch between the placed words and the lyrics' own timing.
    const followItem =
      !app.voice?.supported || !placeable || listening || waiting
        ? []
        : lyrics?.voice
          ? [
              lyrics.voice === "on"
                ? { label: "Use the Lyrics’ Own Timing", icon: "lyrics", action: toggleVoice }
                : { label: "Follow the Singer Again", icon: "lyrics", action: toggleVoice },
            ]
          : [{ label: "Follow the Singer", icon: "lyrics", action: follow }];
    app.openMenu(e, [
      ...followItem,
      ...(timed ? [{ label: "Adjust Timing…", icon: "clock", action: () => (timingOpen = true) }] : []),
      ...(hasSingers ? [{ label: showSingers ? "Hide singers" : "Show singers", icon: "artist", action: toggleSingers }] : []),
      ...(canFind
        ? [{ label: findingSingers ? "Finding who sings…" : app.aiReady ? "Who sings? (AI)" : "Tell the singers apart", icon: "sparkle", action: findSingers }]
        : []),
      { label: timed ? "Search for other lyrics" : "Find timed lyrics", icon: "search", action: openSearch },
      { label: "Paste lyrics", icon: "edit", action: () => (panel = "paste") },
      { label: "Hide these lyrics", icon: "close", action: hideLyrics },
    ]);
  }

  /** An instrumental version: any lyrics are the sung version's, for singing along. */
  const instrumentalTrack = $derived(/\b(instrumental|inst\.?|off[ -]?vocal|karaoke)\b/i.test(track?.title ?? ""));
  const vocals = $derived(1 - (app.dsp?.vocalCut ?? 0));
  /** Sing along: the singer turned down (the level can be fine-tuned while it's on). */
  const singing = $derived((app.dsp?.vocalCut ?? 0) > 0);
  const toggleSinging = () => app.updateDsp((d) => (d.vocalCut = singing ? 0 : 0.85));
  /** Where the lyrics come from, in words, under the last line. */
  const creditLabel: Record<string, string> = {
    embedded: "Lyrics from the song’s file",
    file: "Lyrics from a .lrc file",
    lrclib: "Lyrics from LRCLIB",
    pasted: "Lyrics you pasted",
    ai: "Lyrics found by the AI DJ",
  };
  const credit = $derived(
    [instrumentalTrack ? "An instrumental: the sung version’s lyrics, to sing along" : "", creditLabel[lyrics?.source ?? ""] ?? ""].filter(Boolean).join(" · "),
  );
</script>

<!-- A line's pieces with their readings above (furigana, pinyin). -->
{#snippet withRuby(parts: RubyPart[])}{#each parts as r}{#if r.ruby}<ruby>{r.text}<rt>{r.ruby}</rt></ruby>{:else}{r.text}{/if}{/each}{/snippet}

<div class="wrap">
  {#if panel === "search"}
    <div class="panel">
      <div class="panel-head">
        <h3>Search lyrics</h3>
        <button class="icon-btn" aria-label="Close" onclick={() => (panel = null)}><Icon name="close" size={16} /></button>
      </div>
      <form class="query" onsubmit={(e) => { e.preventDefault(); runSearch(); }}>
        <div class="search">
          <Icon name="search" size={16} />
          <input bind:value={query.title} placeholder="Title" aria-label="Title" />
          <button class="pill" type="submit" disabled={busy === "search"}>{busy === "search" ? "Searching…" : "Search"}</button>
        </div>
        <div class="credits">
          <label class="search small-field"><span>Artist</span><input bind:value={query.artist} placeholder="Any" /></label>
          <label class="search small-field"><span>Album</span><input bind:value={query.album} placeholder="Any" /></label>
        </div>
      </form>
      {#if app.aiReady}
        <button class="ai" onclick={findWithAi} disabled={busy === "ai"}>
          <Icon name="sparkle" size={15} />{busy === "ai" ? "The AI is searching the web…" : "Let the AI find them"}
        </button>
      {/if}
      {#if error}<p class="err">{error}</p>{/if}
      <ul class="hits">
        {#each hits ?? [] as h (h.id)}
          {@const close = track && Math.abs(h.duration - track.duration) <= 4}
          <li>
            <button onclick={() => choose(h)} disabled={busy === "use"}>
              <span class="hit-main">
                <span class="hit-title">{h.title}</span>
                <span class="hit-sub">{h.artist}{h.album ? `, ${h.album}` : ""}</span>
                {#if h.preview}<span class="hit-preview">{h.preview}</span>{/if}
              </span>
              <span class="hit-meta">
                {#if h.synced}<span class="tag synced">Synced</span>{:else if h.instrumental}<span class="tag">Instrumental</span>{:else}<span class="tag">Text only</span>{/if}
                <span class="dur" class:match={close}>{formatTime(h.duration)}</span>
              </span>
            </button>
          </li>
        {:else}
          {#if hits && busy !== "search"}<li class="none">Nothing found. Try fewer words, no album, or the title in another language.</li>{/if}
        {/each}
      </ul>
      <button class="link small" onclick={() => (panel = "paste")}>Paste lyrics instead</button>
    </div>
  {:else if panel === "paste"}
    <div class="panel">
      <div class="panel-head">
        <h3>Paste lyrics</h3>
        <button class="icon-btn" aria-label="Close" onclick={() => (panel = null)}><Icon name="close" size={16} /></button>
      </div>
      <p class="hint">Plain text, or LRC with timings like [01:02.50] to sing along.</p>
      <textarea class="field" rows="14" bind:value={pasted} aria-label="Lyrics" spellcheck="false"></textarea>
      {#if error}<p class="err">{error}</p>{/if}
      <div class="row">
        <button class="pill" onclick={savePasted} disabled={!pasted.trim()}>Use these lyrics</button>
        <button class="pill ghost" onclick={() => (panel = "search")}>Back to search</button>
      </div>
    </div>
  {:else if panel === "voice" && app.voice}
    {@const v = app.voice}
    <div class="panel">
      <div class="panel-head">
        <h3>Follow the singer</h3>
        <button class="icon-btn" aria-label="Close" onclick={() => (panel = null)}><Icon name="close" size={16} /></button>
      </div>
      <p class="hint">
        Cadence can listen to a song, separate the singing from the music, and place every word of the lyrics on the singing,
        so the highlight moves with the singer, syllable by syllable. It runs on this computer ({voiceSpeed}), and each song
        is listened to only once.
      </p>
      <p class="hint">It needs a one-time download of {Math.round(v.downloadSize / 1e6)} MB: ONNX Runtime, the MDX-Net vocal separator and Meta’s MMS aligner.</p>
      {#if v.installing}
        <p class="hint">Downloading… {Math.round(v.installing[0] / 1e6)} of {Math.round(v.installing[1] / 1e6)} MB</p>
      {:else}
        {#if v.error}<p class="err">{v.error}</p>{/if}
        <div class="row">
          <button class="pill" onclick={downloadVoice}><Icon name="download" size={15} />Download and follow</button>
          <button class="pill ghost" onclick={() => (panel = null)}>Not now</button>
        </div>
      {/if}
    </div>
  {:else}
    <div class="lyrics" class:furi={rubyOn} role="presentation" bind:this={box} onwheel={markUser} ontouchmove={markUser} onkeydown={onKey} onscroll={onScroll} onscrollend={hasScrollEnd ? onScrollEnd : undefined}>
      {#if loading}
        <p class="status">Looking for lyrics…</p>
      {:else if lyrics?.instrumental}
        <p class="status big">Instrumental</p>
      {:else if lyrics?.synced.length}
        <div class="pad"></div>
        {#each lyrics.synced as line, i}
          <button class="line" class:active={i === active} class:past={i < active} class:gap={!line.text} style="--c: {singerColor(line)}; --ct: {tint(singerColor(line))}" onclick={() => app.seek(Math.max(0, line.time + offset))}>
            {#if singerLabels[i]}<span class="who"><i></i>{singerLabels[i]}</span>{/if}
            {#if i === active && line.text}
              {#each sweepRuby as p}<span class="w" style="--p: {progress(p) * 100}%">{@render withRuby(p.parts)}</span>{/each}
            {:else if line.text && rubyLines[i]}
              {@render withRuby(rubyLines[i] ?? [])}
            {:else}
              {line.text || "♪"}
            {/if}
            {#if roman[i]}
              <span class="roman">
                {#if i === active}
                  {#each romanWords(roman[i]) as w}<span class="w" style="--p: {wordFill(w)}%">{w.text}</span>{/each}
                {:else}
                  {roman[i]}
                {/if}
              </span>
            {/if}
            {#if translatedLines[i] && line.text}<span class="tr">{translatedLines[i]}</span>{/if}
          </button>
        {/each}
        {#if credit}<p class="credit">{credit}</p>{/if}
        <div class="pad"></div>
      {:else if lyrics?.plain}
        {#if roman.length || translatedLines.length || rubyLines.some(Boolean)}
          <div class="plain">
            {#each plainLines as text, i}<p>{#if rubyLines[i]}{@render withRuby(rubyLines[i] ?? [])}{:else}{text}{/if}{#if roman[i]}<span class="roman">{roman[i]}</span>{/if}{#if translatedLines[i] && text.trim()}<span class="tr">{translatedLines[i]}</span>{/if}</p>{/each}
          </div>
        {:else}
          <p class="plain">{lyrics.plain}</p>
        {/if}
        {#if credit}<p class="credit">{credit}</p>{/if}
      {:else}
        <div class="status">
          {#if lyrics?.lookup === "hidden"}
            <p class="big">Lyrics hidden</p>
            <p>You marked this song’s lyrics as wrong. Search for the right ones, or paste them.</p>
          {:else if lyrics?.lookup === "unavailable"}
            <p class="big">Lyrics service is busy</p>
            <p>LRCLIB couldn’t be reached just now. Try searching, or Cadence will look again next time.</p>
          {:else}
            <p class="big">No lyrics found yet</p>
            <p>Search for them, let the AI find them, or paste your own.</p>
          {/if}
          <div class="actions">
            {#if app.aiReady}
              <button class="pill" onclick={findWithAi} disabled={busy === "ai"}><Icon name="sparkle" size={15} />{busy === "ai" ? "Searching the web…" : "Find with AI"}</button>
            {/if}
            <button class="pill" class:ghost={app.aiReady} onclick={openSearch}><Icon name="search" size={15} />Search</button>
            <button class="pill ghost" onclick={() => (panel = "paste")}><Icon name="edit" size={15} />Paste</button>
          </div>
          {#if error}<p class="err">{error}</p>{/if}
        </div>
      {/if}
    </div>
    <!-- What's used often stays here: sing along, Latin letters, and anything going on (listening
         to the song, a timing nudge). The rest is in ⋯. -->
    <div class="foot">
      <div class="tools">
        {#if lyrics?.synced.length && app.dsp && !instrumentalTrack}
          <button class="chip" class:on={singing} aria-pressed={singing} onclick={toggleSinging} title="Turn the singer down to sing along yourself">
            <Icon name="song" size={14} />Sing along
          </button>
          {#if singing}
            <input type="range" class="slider level" min="0" max="0.9" step="0.05" value={vocals}
              style="--pct: {(vocals / 0.9) * 100}%; --fill: white"
              oninput={(e) => app.updateDsp((d) => (d.vocalCut = Math.max(0.1, 1 - +e.currentTarget.value)))}
              aria-label="How much of the singer is left" title="How much of the singer is left: {Math.round(vocals * 100)}%" />
          {/if}
        {/if}
        {#if romanName && lyrics && (lyrics.synced.length || lyrics.plain)}
          <button class="chip" class:on={pronounce} aria-pressed={pronounce} onclick={togglePronounce} title="Show the lyrics in Latin letters">
            <span class="aa">Aa</span>{romanName}
          </button>
        {/if}
        {#if rubyName && hasText}
          <button class="chip" class:on={furigana} aria-pressed={furigana} onclick={toggleFurigana}
            title={rubyName === "Furigana" ? "Show how the kanji are read, above them" : "Show each character’s pinyin above it"}>
            <span class="aa">{rubyName === "Furigana" ? "ふり" : "pīn"}</span>{rubyName}
          </button>
        {/if}
        {#if hasText}
          <button class="chip" class:on={translateOn} class:busy={translateState === "translating"} aria-pressed={translateOn} onclick={toggleTranslate}
            title={translateState === "needs-ai" ? "Translations come with some lyrics; for the others Cadence asks the AI DJ, which isn’t set up" : translateState === "same" ? `These lyrics are in ${LANG_NAMES[translateLang]} already` : translateState === "error" ? translateError : `Show a translation into ${LANG_NAMES[translateLang]} under each line`}>
            <span class="aa">文A</span>{translateState === "translating" ? "Translating…" : "Translation"}
          </button>
          {#if translateOn}
            <select class="lang" aria-label="Translate into" value={translateLang} onchange={(e) => setTranslateLang(e.currentTarget.value)}>
              {#each Object.entries(LANG_NAMES) as [code, name] (code)}<option value={code}>{name}</option>{/each}
            </select>
            {#if translateState === "needs-ai"}
              <button class="chip warn" onclick={openAiSettings} title="Turn on the AI DJ in Settings to translate lyrics that don’t come with a translation">Needs the AI DJ: Settings</button>
            {:else if translateState === "error"}
              <span class="chip warn" title={translateError}>Couldn’t translate</span>
            {/if}
          {/if}
        {/if}
        {#if app.voice?.supported && placeable}
          {#if listening}
            <span class="chip busy" title="Cadence is listening to this song once, to place its lyrics on the singing">
              <Icon name="lyrics" size={14} />Following the singer <span class="pct">{Math.round((listening.done / Math.max(listening.total, 1)) * 100)}%</span>
            </span>
          {:else if waiting}
            <span class="chip busy" title="Cadence is listening to another song first"><Icon name="lyrics" size={14} />Waiting…</span>
          {:else if voiceError && lyricsMismatch}
            <!-- Trying again gives the same answer: the lyrics are the problem (often another version's). -->
            <button class="chip warn" onclick={openSearch} title="Most of these lyrics aren’t in the singing: they may be another version’s (an acoustic or live one sings other lines). Search for this version’s lyrics.">
              <Icon name="search" size={14} />Lyrics don’t match this version
            </button>
          {:else if voiceError}
            <button class="chip warn" onclick={follow} title={voiceError}><Icon name="lyrics" size={14} />Couldn’t follow the singer: try again</button>
          {/if}
        {/if}
        {#if lyrics?.synced.length && (timingOpen || offset !== 0)}
          <span class="timing" title="Lyrics early or late for this song? Nudge them.">
            <button class="nudge" onclick={() => nudge(-0.25)} aria-label="Lyrics earlier" title="Earlier">−</button>
            <button class="value" onclick={() => nudge(0)} title="Back to the lyrics' own timing">
              {offset === 0 ? "Timing" : `${offset > 0 ? "+" : ""}${offset.toFixed(2)} s`}
            </button>
            <button class="nudge" onclick={() => nudge(0.25)} aria-label="Lyrics later" title="Later">+</button>
            {#if offset === 0}
              <button class="nudge" onclick={() => (timingOpen = false)} aria-label="Close timing" title="Done"><Icon name="close" size={12} /></button>
            {/if}
          </span>
        {/if}
      </div>
      {#if lyrics && !lyrics.instrumental && (lyrics.synced.length || lyrics.plain)}
        <button class="chip round more" onclick={moreMenu} aria-label="Lyrics options" title="Lyrics options: follow the singer, timing, other lyrics, paste, hide">
          <Icon name="more" size={16} />
        </button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .wrap {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .lyrics {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 8px;
    mask-image: linear-gradient(to bottom, transparent, black 14%, black 86%, transparent);
    scrollbar-width: none;
  }
  .lyrics::-webkit-scrollbar {
    display: none;
  }
  .pad {
    height: 38vh;
  }
  .line {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 12px;
    margin: 2px 0;
    border-radius: var(--r-md);
    font-family: var(--display);
    font-size: clamp(22px, 2.4vw, 34px);
    font-weight: 700;
    line-height: 1.25;
    letter-spacing: -0.015em;
    /* Upcoming lines are readable (about 5:1 on the darkened backdrop) so the eye can read ahead;
       sung lines step back further. */
    color: rgba(var(--ct, 255, 255, 255), 0.6);
    transition:
      color 0.35s,
      transform 0.35s;
    transform-origin: left center;
  }
  .line:hover {
    color: rgba(var(--ct, 255, 255, 255), 0.85);
    background: rgba(255, 255, 255, 0.06);
  }
  .line.past {
    color: rgba(var(--ct, 255, 255, 255), 0.38);
  }
  .line.active {
    transform: scale(1.02);
  }
  @media (prefers-reduced-motion: reduce) {
    .line,
    .line.active {
      transition: none;
      transform: none;
    }
  }
  /* Each word fills from left to right as it's sung. */
  .w {
    color: transparent;
    background: linear-gradient(90deg, rgb(var(--c, 255, 255, 255)) var(--p), rgba(var(--ct, 255, 255, 255), 0.6) var(--p));
    -webkit-background-clip: text;
    background-clip: text;
    white-space: pre-wrap;
  }
  /* Who sings, where the singer changes: a small name in the singer's colour. */
  .who {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 2px;
    font-family: var(--sans, inherit);
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: rgba(var(--c, 255, 255, 255), 0.75);
  }
  .who i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: rgb(var(--c, 255, 255, 255));
  }
  .line.active .who {
    color: rgb(var(--c, 255, 255, 255));
  }
  .line.gap {
    font-size: 20px;
  }
  .plain {
    white-space: pre-wrap;
    font-family: var(--display);
    font-size: 22px;
    font-weight: 600;
    line-height: 1.6;
    color: rgba(255, 255, 255, 0.85);
    padding: 10vh 12px;
    user-select: text;
  }
  .status {
    padding: 28vh 12px 0;
    color: rgba(255, 255, 255, 0.65);
    max-width: 46ch;
  }
  .status p {
    margin: 0 0 8px;
  }
  .big {
    font-family: var(--display);
    font-size: 26px;
    font-weight: 700;
    color: rgba(255, 255, 255, 0.9);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 18px;
  }
  /* One calm row: the lyrics' tools on the left, the vocal level on the right. Nothing in it
     wraps onto a second line; on a narrow window the vocal level moves under the tools. */
  .foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px 14px;
    padding: 10px 12px 18px;
    font-size: 12.5px;
    color: rgba(255, 255, 255, 0.5);
    min-height: 48px;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  /* Readings above the characters: the room for them is kept on every line while they're on (not
     just the sung one), so no line grows or shrinks as the song plays. */
  .furi .line,
  .furi .plain {
    line-height: 2.05;
  }
  .furi .line {
    padding-top: 4px;
  }
  ruby {
    ruby-position: over;
  }
  rt {
    font-family: var(--sans, inherit);
    font-size: 0.42em;
    font-weight: 600;
    letter-spacing: 0;
    line-height: 1;
    color: rgba(var(--ct, 255, 255, 255), 0.75);
  }
  /* In the sung line the base text is drawn by the sweep; its reading stays plainly readable. */
  .w rt {
    -webkit-text-fill-color: rgba(255, 255, 255, 0.85);
    color: rgba(255, 255, 255, 0.85);
  }
  /* The translation under each line: like the pronunciation, a little dimmer, never swept. */
  .tr {
    display: block;
    margin-top: 3px;
    font-family: var(--sans, inherit);
    font-size: 0.5em;
    font-weight: 500;
    letter-spacing: 0;
    line-height: 1.35;
    opacity: 0.72;
  }
  .line.active .tr {
    color: rgba(255, 255, 255, 0.8);
  }
  .lang {
    height: 28px;
    padding: 0 6px;
    border-radius: 99px;
    border: 0;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.8);
    background: rgba(255, 255, 255, 0.08);
  }
  .lang option {
    color: #10111a;
  }
  /* The pronunciation under each line: smaller and lighter, but following the line's state. */
  .roman {
    display: block;
    margin-top: 3px;
    font-family: var(--sans, inherit);
    font-size: 0.52em;
    font-weight: 500;
    letter-spacing: 0;
    line-height: 1.35;
    opacity: 0.82;
  }
  .line.active .roman {
    color: rgba(255, 255, 255, 0.85);
  }
  .plain p {
    margin: 0 0 10px;
  }
  .timing {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 28px;
    padding: 0 3px;
    white-space: nowrap;
    border-radius: 99px;
    background: rgba(255, 255, 255, 0.08);
  }
  .timing .nudge {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    font-size: 15px;
    line-height: 1;
    color: rgba(255, 255, 255, 0.8);
  }
  .timing .nudge:hover {
    background: rgba(255, 255, 255, 0.14);
  }
  .timing .value {
    min-width: 50px;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.75);
    font-variant-numeric: tabular-nums;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 11px 0 8px;
    border-radius: 99px;
    font-size: 12.5px;
    color: rgba(255, 255, 255, 0.75);
    background: rgba(255, 255, 255, 0.08);
    transition: background 0.15s, color 0.15s;
  }
  .chip:hover {
    background: rgba(255, 255, 255, 0.14);
  }
  .chip.on {
    color: #10111a;
    background: rgba(255, 255, 255, 0.9);
  }
  .aa {
    font-weight: 700;
    font-size: 11px;
    padding: 1px 4px;
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.14);
  }
  .chip.on .aa {
    background: rgba(16, 17, 26, 0.12);
  }
  .chip {
    white-space: nowrap;
  }
  .chip.round {
    width: 28px;
    padding: 0;
    justify-content: center;
  }
  .chip.warn {
    color: var(--warn);
  }
  .pct {
    display: inline-block;
    min-width: 3ch;
    text-align: right;
  }
  /* Working (translating, following the singer, waiting): said in words (and a percentage), and
     dimmed; no pulsing, which ran at the display's rate for as long as a job took (unbounded if
     a request hung). */
  .chip.busy {
    cursor: default;
    font-variant-numeric: tabular-nums;
    opacity: 0.72;
  }
  /* How much of the singer is left, while singing along. */
  .level {
    width: 80px;
  }
  /* Lyrics options, at the end of the row. */
  .more {
    margin-left: auto;
    flex: none;
  }
  /* Where the lyrics come from, under the last line. */
  .credit {
    margin: 18px 12px 0;
    font-size: 12.5px;
    color: rgba(255, 255, 255, 0.6);
  }
  .small {
    font-size: 12.5px;
  }
  .panel .link {
    align-self: flex-start;
  }
  .err {
    color: var(--warn);
    font-size: 13px;
    margin: 10px 0 0;
  }
  .panel {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 24px 12px 20px;
    max-width: 640px;
  }
  .panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .panel h3 {
    margin: 0;
    font-family: var(--display);
    font-size: 24px;
    font-weight: 700;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 5px 5px 14px;
    border-radius: 999px;
    background: rgba(0, 0, 0, 0.25);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.14);
    color: rgba(255, 255, 255, 0.7);
  }
  .search input {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    outline: none;
    font-size: 14.5px;
    color: white;
    user-select: text;
  }
  .query {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  /* Artist and album: smaller fields under the title, in the same glass. */
  .credits {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .search.small-field {
    flex: 1 1 180px;
    min-width: 0;
    padding: 6px 14px;
    gap: 8px;
  }
  .small-field span {
    flex: none;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.5);
  }
  .small-field input {
    font-size: 13.5px;
  }
  .ai {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13.5px;
    font-weight: 600;
    color: var(--accent);
  }
  .hits {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    flex: 1;
    min-height: 0;
  }
  .hits button {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 9px 10px;
    border-radius: var(--r-md);
    text-align: left;
  }
  .hits button:hover {
    background: rgba(255, 255, 255, 0.07);
  }
  .hit-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .hit-title {
    font-weight: 600;
  }
  .hit-sub,
  .hit-preview {
    font-size: 12.5px;
    color: rgba(255, 255, 255, 0.55);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hit-preview {
    font-style: italic;
    color: rgba(255, 255, 255, 0.42);
  }
  .hit-meta {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
    font-size: 12px;
  }
  .tag {
    padding: 2px 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.6);
  }
  .tag.synced {
    background: color-mix(in srgb, var(--accent) 30%, transparent);
    color: white;
  }
  .dur {
    color: rgba(255, 255, 255, 0.45);
    font-variant-numeric: tabular-nums;
  }
  .dur.match {
    color: var(--good);
  }
  .none {
    padding: 14px 10px;
    color: rgba(255, 255, 255, 0.55);
    font-size: 13px;
  }
  .hint {
    margin: 0;
    font-size: 13px;
    color: rgba(255, 255, 255, 0.55);
  }
  textarea.field {
    flex: 1;
    min-height: 200px;
    resize: none;
    font-family: inherit;
    font-size: 14px;
    line-height: 1.5;
    color: white;
    background: rgba(0, 0, 0, 0.25);
  }
  .row {
    display: flex;
    gap: 10px;
  }
  @media (max-width: 720px) {
    .line {
      font-size: 24px;
      padding: 6px 8px;
    }
    .pad {
      height: 24vh;
    }
    .foot {
      gap: 8px 14px;
      padding: 6px 8px 8px;
    }
    .tools {
      flex-wrap: wrap;
    }
  }
</style>

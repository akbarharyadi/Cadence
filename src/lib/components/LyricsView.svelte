<script lang="ts">
  import { api, formatTime, type LyricLine, type Lyrics, type LyricsHit } from "$lib/api";
  import { app } from "$lib/store.svelte";
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
      .finally(() => (loading = false));
  });

  /** Index of the line being sung (slightly early, so the eye gets there in time). */
  const active = $derived.by(() => {
    const lines = lyrics?.synced ?? [];
    const t = app.position + 0.25;
    let i = -1;
    for (let k = 0; k < lines.length && lines[k].time <= t; k++) i = k;
    return i;
  });

  // Keep the current line in the middle, unless the user has just scrolled to read ahead.
  $effect(() => {
    const i = active;
    if (i < 0 || !box || Date.now() - userScrolledAt < 4000) return;
    const el = box.querySelectorAll<HTMLElement>(".line")[i];
    el?.scrollIntoView({ behavior: "smooth", block: "center" });
  });

  // ---------- karaoke sweep ----------

  interface Piece {
    text: string;
    start: number;
    end: number;
  }

  /**
   * The words of a line with their timing. Word-timed lyrics use their own timings; otherwise the
   * line's time is shared out by length (per character for text without spaces, like Chinese),
   * and a line never sweeps for longer than it takes to sing it.
   */
  function pieces(line: LyricLine, next: LyricLine | undefined): Piece[] {
    if (line.words?.length) {
      return line.words.map((w, i) => ({
        text: w.text,
        start: w.time,
        end: line.words![i + 1]?.time ?? Math.min(next?.time ?? w.time + 1, w.time + 1.5),
      }));
    }
    const parts = /\s/.test(line.text) ? line.text.split(/(?<=\s)/) : [...line.text];
    const chars = parts.reduce((n, p) => n + p.trim().length, 0) || 1;
    const end = Math.min(next?.time ?? line.time + 4, line.time + Math.max(1.6, chars * 0.16));
    const span = Math.max(0.3, end - line.time - 0.15);
    let t = line.time;
    return parts.map((p) => {
      const d = (span * Math.max(p.trim().length, 0.5)) / chars;
      const piece = { text: p, start: t, end: t + d };
      t += d;
      return piece;
    });
  }

  const sweep = $derived(active >= 0 && lyrics ? pieces(lyrics.synced[active], lyrics.synced[active + 1]) : []);
  const progress = (p: Piece) => Math.min(1, Math.max(0, (app.position + 0.1 - p.start) / (p.end - p.start)));

  // ---------- find / fix lyrics ----------

  let panel = $state<"search" | "paste" | null>(null);
  let query = $state("");
  let hits = $state<LyricsHit[] | null>(null);
  let busy = $state<"" | "search" | "ai" | "use">("");
  let error = $state("");
  let pasted = $state("");

  function openSearch() {
    panel = "search";
    error = "";
    if (!query && track) {
      const title = track.title.split(/[(\[（【]/)[0].trim();
      const latin = title.replace(/[^\x00-\x7F]/g, "").trim();
      query = latin.replace(/[^A-Za-z0-9]/g, "").length >= 2 ? latin : title;
    }
    if (!hits) runSearch();
  }

  async function runSearch() {
    if (!query.trim()) return;
    busy = "search";
    error = "";
    try {
      hits = await api.searchLyrics(query);
    } catch (e) {
      error = String(e);
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

  const sourceLabel: Record<string, string> = {
    embedded: "From the file",
    file: "From a .lrc file",
    lrclib: "From LRCLIB",
    pasted: "Pasted by you",
    ai: "Found by the AI DJ",
  };
  /** An instrumental version: any lyrics are the sung version's, for singing along. */
  const instrumentalTrack = $derived(/\b(instrumental|inst\.?|off[ -]?vocal|karaoke)\b/i.test(track?.title ?? ""));
  const vocals = $derived(1 - (app.dsp?.vocalCut ?? 0));
</script>

<div class="wrap">
  {#if panel === "search"}
    <div class="panel">
      <div class="panel-head">
        <h3>Search lyrics</h3>
        <button class="icon-btn" aria-label="Close" onclick={() => (panel = null)}><Icon name="close" size={16} /></button>
      </div>
      <form class="search" onsubmit={(e) => { e.preventDefault(); runSearch(); }}>
        <Icon name="search" size={16} />
        <input bind:value={query} placeholder="Title, artist or a line from the song" aria-label="Search lyrics" />
        <button class="pill" type="submit" disabled={busy === "search"}>{busy === "search" ? "Searching…" : "Search"}</button>
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
          {#if hits && busy !== "search"}<li class="none">Nothing found. Try fewer words, or the title in another language.</li>{/if}
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
  {:else}
    <div class="lyrics" bind:this={box} onwheel={() => (userScrolledAt = Date.now())}>
      {#if loading}
        <p class="status">Looking for lyrics…</p>
      {:else if lyrics?.instrumental}
        <p class="status big">Instrumental</p>
      {:else if lyrics?.synced.length}
        <div class="pad"></div>
        {#each lyrics.synced as line, i}
          <button class="line" class:active={i === active} class:past={i < active} class:gap={!line.text} onclick={() => app.seek(line.time)}>
            {#if i === active && line.text}
              {#each sweep as p}<span class="w" style="--p: {progress(p) * 100}%">{p.text}</span>{/each}
            {:else}
              {line.text || "♪"}
            {/if}
          </button>
        {/each}
        <div class="pad"></div>
      {:else if lyrics?.plain}
        <p class="plain">{lyrics.plain}</p>
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
    <div class="foot">
      {#if instrumentalTrack && (lyrics?.synced.length || lyrics?.plain)}
        <span class="sing-along"><Icon name="song" size={14} />Sing along: lyrics from the sung version</span>
      {:else if lyrics?.source}<span class="source">{sourceLabel[lyrics.source] ?? ""}</span>{/if}
      {#if lyrics && !lyrics.instrumental && !lyrics.synced.length && lyrics.plain}
        <button class="link small" onclick={openSearch} title="These lyrics have no timing, so they can't follow the song">Find timed lyrics</button>
      {:else if lyrics && !lyrics.instrumental && lyrics.synced.length}
        <button class="link small" onclick={openSearch}>Wrong lyrics?</button>
      {/if}
      {#if lyrics && (lyrics.synced.length || lyrics.plain)}
        <button class="link small" onclick={hideLyrics} title="Don’t show these lyrics for this song again">Hide these lyrics</button>
      {/if}
      {#if lyrics?.synced.length && app.dsp && !instrumentalTrack}
        <label class="sing" title="Turn the singer down to sing along yourself">
          <Icon name="song" size={15} />
          <span>Vocals</span>
          <input type="range" class="slider" min="0" max="1" step="0.05" value={vocals}
            style="--pct: {vocals * 100}%; --fill: white; width: 110px"
            oninput={(e) => app.updateDsp((d) => (d.vocalCut = 1 - +e.currentTarget.value))} aria-label="Vocal level" />
          <span class="num">{Math.round(vocals * 100)}%</span>
        </label>
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
    color: rgba(255, 255, 255, 0.32);
    transition:
      color 0.35s,
      transform 0.35s,
      filter 0.35s;
    transform-origin: left center;
    filter: blur(0.6px);
  }
  .line:hover {
    color: rgba(255, 255, 255, 0.6);
    background: rgba(255, 255, 255, 0.05);
    filter: none;
  }
  .line.past {
    color: rgba(255, 255, 255, 0.45);
  }
  .line.active {
    filter: none;
    transform: scale(1.02);
  }
  /* Each word fills from left to right as it's sung. */
  .w {
    color: transparent;
    background: linear-gradient(90deg, #fff var(--p), rgba(255, 255, 255, 0.38) var(--p));
    -webkit-background-clip: text;
    background-clip: text;
    white-space: pre-wrap;
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
  .foot {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 10px 12px 18px;
    font-size: 12.5px;
    color: rgba(255, 255, 255, 0.5);
    min-height: 44px;
  }
  .foot .link {
    color: rgba(255, 255, 255, 0.7);
  }
  .sing-along {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: rgba(255, 255, 255, 0.8);
  }
  .sing {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 8px;
    color: rgba(255, 255, 255, 0.75);
  }
  .num {
    width: 34px;
    font-variant-numeric: tabular-nums;
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
      flex-wrap: wrap;
      gap: 8px 14px;
      padding: 6px 8px 8px;
    }
    .sing {
      margin-left: 0;
    }
  }
</style>

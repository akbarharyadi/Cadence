<script lang="ts">
  // Fix Album Info: find the album in the iTunes and Deezer catalogues, check which of its songs
  // each file is (lined up by length and kind of recording; any can be changed), and write the
  // album's info, cover and each song's own title, artist and number at once.
  import { api, formatTime, type AlbumMatch, type AlbumPlan, type SongAssign } from "#lib/api.js";
  import { app } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import Select from "./Select.svelte";

  const album = $derived(app.fixingAlbum!);
  // Songs of a single-file album can't be written one by one (the menu doesn't offer those).
  const tracks = $derived(
    app
      .albumTracks(album)
      .filter((t) => !t.cue)
      .sort((a, b) => (a.discNo ?? 0) - (b.discNo ?? 0) || (a.trackNo ?? 0) - (b.trackNo ?? 0) || a.title.localeCompare(b.title)),
  );
  let query = $state("");
  let results = $state<AlbumMatch[]>([]);
  let picked = $state<AlbumMatch | null>(null);
  let plan = $state<AlbumPlan | null>(null);
  let assign = $state<SongAssign[]>([]);
  let busy = $state<"search" | "plan" | "save" | null>(null);
  let error = $state("");
  /** Keep the songs' own titles (null: not chosen, so on when the catalogue's are in another script). */
  let keepTitles = $state<boolean | null>(null);

  const cjk = (s: string) => /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u.test(s);
  const keeping = $derived(
    keepTitles ??
      (!!plan &&
        assign.some((a) => {
          const t = tracks.find((t) => t.id === a.trackId);
          const s = a.song != null ? plan!.songs[a.song] : null;
          return !!t && !!s && cjk(t.title) !== cjk(s.title);
        })),
  );

  async function search(q?: string) {
    busy = "search";
    error = "";
    try {
      const r = await api.findAlbumInfo(album.id, q);
      query = r.term;
      results = r.results;
      if (!r.results.length) error = "Nothing found. Try other words, like the artist and the album name.";
      else if (!r.results.some((c) => c.likely))
        error = "None of these albums has songs as long as yours; it may not be in the catalogues. Pick one only if it's right.";
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }
  search();

  async function pick(c: AlbumMatch) {
    picked = c;
    busy = "plan";
    error = "";
    try {
      plan = await api.albumPlan(album.id, c.albumRef);
      assign = plan.assign;
      keepTitles = null;
    } catch (e) {
      error = String(e);
      picked = null;
    } finally {
      busy = null;
    }
  }

  function back() {
    picked = null;
    plan = null;
    error = "";
  }

  const songOptions = $derived([
    { value: "", label: "Not on this album (keeps its own title)" },
    ...(plan?.songs ?? []).map((s, i) => ({ value: String(i), label: `${s.trackNo ?? i + 1}. ${s.title} (${formatTime(s.duration)})` })),
  ]);
  const setSong = (trackId: string, v: string) => (assign = assign.map((a) => (a.trackId === trackId ? { ...a, song: v === "" ? null : Number(v) } : a)));
  const matched = $derived(assign.filter((a) => a.song != null).length);

  async function save() {
    if (!picked || !plan) return;
    busy = "save";
    error = "";
    try {
      const info = { ...($state.snapshot(picked) as AlbumMatch), year: picked.year ?? plan.year, genre: picked.genre || plan.genre };
      const r = await api.saveAlbumInfo(album.id, info, $state.snapshot(plan.songs), $state.snapshot(assign), keeping);
      const songs = (n: number) => `${n} ${n === 1 ? "song" : "songs"}`;
      app.showToast(
        `Saved “${info.title}” (${songs(r.songs)})${r.later ? ". The playing song’s file is updated when it finishes." : ""}`,
        app.undoAction(r.change),
      );
      close();
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  const close = () => (app.fixingAlbum = null);
</script>

<Modal title="Fix album info" onclose={close} width="680px">
  <p class="now">
    <span class="label">Now</span>
    <span class="cur">{album.title}</span>
    <span class="secondary">{album.artist}, {tracks.length} {tracks.length === 1 ? "song" : "songs"}</span>
  </p>

  {#if !plan}
    <form class="search" onsubmit={(e) => { e.preventDefault(); search(query); }}>
      <Icon name="search" size={16} />
      <input class="field" bind:value={query} placeholder="Artist and album name" aria-label="Search the catalogue" />
      <button class="pill ghost" type="submit" disabled={busy !== null}>Search</button>
    </form>
    <div class="results" role="listbox" aria-label="Albums" aria-busy={busy !== null}>
      {#if busy === "search"}
        {#each [0, 1, 2] as i (i)}<div class="hit ghost-row"><span class="art"></span><span class="lines"><i></i><i></i></span></div>{/each}
      {:else}
        {#each results as c, i (i)}
          <button class="hit" class:on={picked === c} role="option" aria-selected={picked === c} disabled={busy !== null} onclick={() => pick(c)}>
            <span class="art">{#if c.thumb}<img src={c.thumb} alt="" loading="lazy" />{/if}</span>
            <span class="lines">
              <span class="title">{c.title}</span>
              <span class="sub">{c.artist}{c.year ? ` · ${c.year}` : ""} · {c.tracks} {c.tracks === 1 ? "song" : "songs"}{c.matched ? ` · ${c.matched} of yours match` : ""}</span>
            </span>
            {#if c.likely}<span class="match">Match</span>{/if}
            <span class="src" title="From the {c.source} catalogue">{c.source}</span>
          </button>
        {/each}
      {/if}
    </div>
  {:else if picked}
    <div class="picked">
      <span class="art big">{#if picked.thumb}<img src={picked.thumb} alt="" />{/if}</span>
      <span class="lines">
        <span class="title">{picked.title}</span>
        <span class="sub">{picked.artist}{picked.year ?? plan.year ? ` · ${picked.year ?? plan.year}` : ""}{picked.genre || plan.genre ? ` · ${picked.genre || plan.genre}` : ""} · {picked.source}</span>
      </span>
      <button class="link" onclick={back}>Another album…</button>
    </div>
    {#if matched === 0}
      <p class="error">None of your songs is as long as a song of this album, so it's probably another album. Saving gives them its name, year and cover anyway.</p>
    {:else}
      <p class="secondary hint">Which song of the album each file is ({matched} of {tracks.length} found). Change any that are wrong.</p>
    {/if}
    <div class="songs">
      {#each tracks as t (t.id)}
        {@const a = assign.find((a) => a.trackId === t.id)}
        <div class="song">
          <span class="mine">
            <span class="title">{t.title}</span>
            <span class="sub">{formatTime(t.duration)}</span>
          </span>
          <Icon name="chevron" size={14} />
          <Select label="Catalogue song for {t.title}" options={songOptions} value={a?.song == null ? "" : String(a.song)} onchange={(v) => setSong(t.id, v)} width="300px" />
        </div>
      {/each}
    </div>
    <label class="keep">
      <input type="checkbox" checked={keeping} onchange={(e) => (keepTitles = e.currentTarget.checked)} />
      <span>Keep the songs’ own titles <span class="secondary">(the catalogue’s are kept as subtitles, so search finds both)</span></span>
    </label>
  {/if}
  {#if busy === "plan"}<p class="secondary hint">Reading the album’s songs…</p>{/if}
  {#if error}<p class="error">{error}</p>{/if}

  {#snippet footer()}
    <button class="pill ghost" onclick={close}>Cancel</button>
    <button class="pill" onclick={save} disabled={!plan || busy !== null}>{busy === "save" ? "Saving…" : plan && matched === 0 ? "Save anyway" : "Save"}</button>
  {/snippet}
</Modal>

<style>
  .now {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin: 0 0 14px;
    font-size: 13.5px;
    min-width: 0;
  }
  .label {
    color: var(--text-3);
    font-size: 12.5px;
  }
  .cur {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .now .secondary {
    white-space: nowrap;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-3);
  }
  .search .field {
    flex: 1;
    font-size: 14px;
    padding: 8px 11px;
  }
  .results {
    margin-top: 12px;
    max-height: min(420px, 50vh);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .hit {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 7px 10px 7px 7px;
    border-radius: var(--r-sm);
    text-align: left;
    min-width: 0;
  }
  .hit:hover {
    background: var(--hover);
  }
  .hit.on {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }
  .art {
    width: 46px;
    height: 46px;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface);
  }
  .art.big {
    width: 64px;
    height: 64px;
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .lines,
  .mine {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .title,
  .sub {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title {
    font-weight: 600;
    font-size: 14px;
  }
  .sub {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .match {
    flex: none;
    font-size: 11.5px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 99px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  .src {
    flex: none;
    font-size: 11px;
    color: var(--text-3);
    padding: 1px 6px;
    border-radius: 99px;
    box-shadow: inset 0 0 0 1px var(--line-strong);
  }
  .picked {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .hint {
    margin: 14px 0 8px;
    font-size: 13px;
  }
  .songs {
    max-height: min(360px, 45vh);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .song {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-3);
    min-width: 0;
  }
  .song .title {
    color: var(--text);
    font-weight: 500;
    font-size: 13.5px;
  }
  .keep {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 12px 0 0;
    font-size: 13px;
  }
  /* Placeholder rows while searching. */
  .ghost-row .art,
  .ghost-row i {
    animation: pulse 1.2s ease-in-out infinite;
  }
  .ghost-row i {
    display: block;
    height: 10px;
    width: 60%;
    border-radius: 5px;
    background: var(--surface);
  }
  .ghost-row i + i {
    width: 40%;
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
  .error {
    margin: 12px 0 0;
    color: var(--warn);
    font-size: 13px;
  }
</style>

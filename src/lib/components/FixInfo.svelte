<script lang="ts">
  // Fix Song Info: find the song in the iTunes and Deezer catalogues and pick the right match; its title,
  // artist, album, year and cover are written into the file (and the album's into its other songs).
  import { api, formatTime, type SongInfo } from "#lib/api.js";
  import { app } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  const track = $derived(app.fixing!);
  let query = $state("");
  let results = $state<SongInfo[]>([]);
  let picked = $state<SongInfo | null>(null);
  let busy = $state<"search" | "save" | null>(null);
  let error = $state("");
  /** Keep the song's own title (null: not chosen, so on when the match's is in another script). */
  let keepTitle = $state<boolean | null>(null);

  const cjk = (s: string) => /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u.test(s);
  const plain = (s: string) => s.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
  // Only a tagged song has a title of its own (an untagged one's is its file name).
  const canKeep = $derived(!!picked && !app.needsInfo(track) && plain(picked.title) !== plain(track.title));
  const keeping = $derived(canKeep && (keepTitle ?? (!!picked && cjk(track.title) !== cjk(picked.title))));
  /** The other songs of its album (tagged ones: untagged songs share their folder's made-up album). */
  const mates = $derived(app.needsInfo(track) ? [] : app.tracks.filter((t) => t.albumId === track.albumId && t.id !== track.id && !app.needsInfo(t)));
  let wholeAlbum = $state(true);

  async function search(q?: string) {
    busy = "search";
    error = "";
    try {
      const r = await api.findSongInfo(track.id, q);
      query = r.term;
      results = r.results;
      picked = r.results.find((c) => c.likely) ?? null;
      if (!r.results.length) error = "Nothing found. Try other words, like the artist and the song title.";
      else if (!picked) error = "None of these is clearly this song; it may not be in the iTunes or Deezer catalogue (demos and unreleased songs often aren't). Pick one only if it's right, or use Edit by hand.";
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }
  search();

  async function save() {
    if (!picked) return;
    busy = "save";
    error = "";
    try {
      const title = keeping ? track.title : picked.title;
      const r = await api.saveSongInfo(track.id, $state.snapshot(picked) as SongInfo, keeping, wholeAlbum && mates.length > 0);
      const songs = (n: number) => `${n} ${n === 1 ? "song" : "songs"}`;
      const album = r.others ? ` and ${songs(r.others)} of its album${r.matched < r.others ? ` (${songs(r.others - r.matched)} not on the catalogue's album got only the album's info)` : ""}` : "";
      app.showToast(r.later ? `Saved “${title}”${album}. Its file is updated when it finishes.` : `Saved “${title}”${album}`);
      close();
    } catch (e) {
      error = String(e);
    } finally {
      busy = null;
    }
  }

  function editByHand() {
    const t = track;
    close();
    app.tagEditing = [t];
  }

  const close = () => (app.fixing = null);
</script>

<Modal title="Fix song info" onclose={close} width="600px">
  <p class="now">
    <span class="label">Now</span>
    <span class="cur">{track.title}</span>
    <span class="secondary">{track.artist}, {formatTime(track.duration)}</span>
  </p>
  <form class="search" onsubmit={(e) => { e.preventDefault(); search(query); }}>
    <Icon name="search" size={16} />
    <input class="field" bind:value={query} placeholder="Artist and song title" aria-label="Search the catalogue" />
    <button class="pill ghost" type="submit" disabled={busy === "search"}>Search</button>
  </form>

  <div class="results" role="listbox" aria-label="Matches" aria-busy={busy === "search"}>
    {#if busy === "search"}
      {#each [0, 1, 2] as i (i)}<div class="hit ghost-row"><span class="art"></span><span class="lines"><i></i><i></i></span></div>{/each}
    {:else}
      {#each results as c, i (i)}
        <button class="hit" class:on={picked === c} role="option" aria-selected={picked === c} onclick={() => (picked = c)} ondblclick={save}>
          <span class="art">{#if c.thumb}<img src={c.thumb} alt="" loading="lazy" />{/if}</span>
          <span class="lines">
            <span class="title">{c.title}</span>
            <span class="sub">{c.artist}, {c.album}{c.year ? ` (${c.year})` : ""}</span>
          </span>
          {#if c.likely}<span class="match">Match</span>{/if}
          {#if c.source}<span class="src" title="From the {c.source} catalogue">{c.source}</span>{/if}
          <span class="len">{formatTime(c.duration)}</span>
        </button>
      {/each}
    {/if}
  </div>
  {#if error}<p class="error">{error}</p>{/if}
  {#if canKeep && picked}
    <label class="keep">
      <input type="checkbox" checked={keeping} onchange={(e) => (keepTitle = e.currentTarget.checked)} />
      <span>Keep my title <b>{track.title}</b> <span class="secondary">instead of “{picked.title}”</span></span>
    </label>
  {/if}
  {#if picked && mates.length}
    <label class="keep">
      <input type="checkbox" bind:checked={wholeAlbum} />
      <span>Also update the other {mates.length === 1 ? "song" : `${mates.length} songs`} on the album <b>{track.album}</b> <span class="secondary">with this album, year and cover</span></span>
    </label>
  {/if}

  {#snippet footer()}
    <button class="link note" onclick={editByHand}>Edit by hand…</button>
    <button class="pill ghost" onclick={close}>Cancel</button>
    <button class="pill" onclick={save} disabled={!picked || busy !== null}>{busy === "save" ? "Saving…" : "Save"}</button>
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
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 55%, transparent);
  }
  .art {
    width: 46px;
    height: 46px;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface);
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .lines {
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
  .keep {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 12px 0 0;
    font-size: 13px;
    min-width: 0;
  }
  .keep > span {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .len {
    flex: none;
    width: 40px;
    text-align: right;
    font-size: 12.5px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
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
  .note {
    margin-right: auto;
    align-self: center;
    font-size: 13px;
  }
</style>

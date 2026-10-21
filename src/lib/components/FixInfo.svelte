<script lang="ts">
  // Fix Song Info: find the song in the iTunes catalogue and pick the right match; its title,
  // artist, album, year and cover are written into the file.
  import { api, formatTime, type SongInfo } from "$lib/api";
  import { app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  const track = $derived(app.fixing!);
  let query = $state("");
  let results = $state<SongInfo[]>([]);
  let picked = $state<SongInfo | null>(null);
  let busy = $state<"search" | "save" | null>(null);
  let error = $state("");

  async function search(q?: string) {
    busy = "search";
    error = "";
    try {
      const r = await api.findSongInfo(track.id, q);
      query = r.term;
      results = r.results;
      picked = r.results.find((c) => c.likely) ?? null;
      if (!r.results.length) error = "Nothing found. Try other words, like the artist and the song title.";
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
      const later = await api.saveSongInfo(track.id, $state.snapshot(picked) as SongInfo);
      app.showToast(later ? `Saved. The file and cover are updated when “${picked.title}” finishes.` : `Saved “${picked.title}”`);
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
          <span class="len">{formatTime(c.duration)}</span>
        </button>
      {/each}
    {/if}
  </div>
  {#if error}<p class="error">{error}</p>{/if}

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

<script lang="ts">
  // Change Cover: covers from iTunes and Deezer for the album (searchable), or an image file.
  // The new cover shows at once and is written into the album's songs.
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, artUrl, type CoverResult } from "$lib/api";
  import { app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  const album = $derived(app.albumById.get(app.coverEditing!));
  const songs = $derived(album?.trackIds.length ?? 0);
  /** Folder-made albums of untagged downloads: one cover for all of them is rarely wanted. */
  const folderAlbum = $derived(album?.artist === "Unknown Artist");
  const isAndroid = navigator.userAgent.includes("Android");

  let query = $state("");
  let results = $state<CoverResult[]>([]);
  let searching = $state(false);
  let saving = $state<string | null>(null);
  let error = $state("");
  /** Many songs: the user confirms once before every file is rewritten. */
  let confirmed = $state(false);
  let pending = $state<{ source: { kind: "url"; url: string } | { kind: "file"; path: string }; key: string } | null>(null);

  async function search(q?: string) {
    searching = true;
    error = "";
    try {
      const r = await api.findCovers(app.coverEditing!, q);
      query = r.term;
      results = r.results;
      if (!r.results.length) error = "No covers found. Try other words, like the artist and the album title.";
    } catch (e) {
      error = String(e);
    } finally {
      searching = false;
    }
  }
  search();

  async function use(source: { kind: "url"; url: string } | { kind: "file"; path: string }, key: string) {
    if (!album) return;
    if (songs > 40 && !confirmed) {
      pending = { source, key };
      return;
    }
    saving = key;
    error = "";
    try {
      await app.setAlbumCover(album, source);
      close();
    } catch (e) {
      error = String(e);
    } finally {
      saving = null;
    }
  }

  async function chooseFile() {
    const path = await open({ title: `Cover for ${album?.title ?? "album"}`, filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "webp", "gif", "bmp"] }] });
    if (typeof path === "string") await use({ kind: "file", path }, "file");
  }

  const close = () => (app.coverEditing = null);
</script>

<Modal title="Cover for {album?.title ?? 'album'}" onclose={close} width="680px">
  {#if album}
    <div class="current">
      <span class="now">{#if album.artSmall}<img src={artUrl(album.artSmall)} alt="" />{:else}<Icon name="album" size={26} />{/if}</span>
      <span class="what">
        <strong>{album.title}</strong>
        <span>{album.artist}, used for {songs === 1 ? "its song" : `all ${songs} songs`}</span>
      </span>
    </div>
    {#if folderAlbum}
      <p class="warn">
        These are untagged songs grouped by their folder. To give one song its own album and cover, use <b>Fix Song Info</b> in its menu
        instead.
      </p>
    {/if}

    <form class="search" onsubmit={(e) => { e.preventDefault(); search(query); }}>
      <Icon name="search" size={16} />
      <input class="field" bind:value={query} placeholder="Artist and album title" aria-label="Search for covers" />
      <button class="pill ghost" type="submit" disabled={searching}>Search</button>
    </form>

    <div class="grid" aria-busy={searching}>
      {#if searching}
        {#each [0, 1, 2, 3, 4, 5, 6, 7] as i (i)}<div class="opt ghost-opt"><span class="pic"></span><i></i></div>{/each}
      {:else}
        {#each results as c, i (i)}
          <button class="opt" class:busy={saving === c.url} disabled={!!saving} onclick={() => use({ kind: "url", url: c.url }, c.url)} title="{c.title}, {c.artist}{c.year ? ` (${c.year})` : ''}, from {c.source}">
            <span class="pic"><img src={c.thumb} alt="" />{#if c.likely}<span class="match">Match</span>{/if}</span>
            <span class="label">{c.title}</span>
            <span class="sub">{c.artist}{c.year ? `, ${c.year}` : ""}</span>
          </button>
        {/each}
      {/if}
    </div>

    {#if pending}
      <div class="confirm" role="alert">
        <span>This rewrites the cover in all {songs} songs’ files.</span>
        <button class="pill ghost small" onclick={() => (pending = null)}>Cancel</button>
        <button class="pill small" onclick={() => { confirmed = true; const p = pending!; pending = null; use(p.source, p.key); }}>Change all {songs}</button>
      </div>
    {/if}
    {#if error}<p class="error">{error}</p>{/if}
  {/if}

  {#snippet footer()}
    <span class="note">Saved into the music files, so it stays after a rescan.</span>
    {#if !isAndroid}<button class="pill ghost" onclick={chooseFile} disabled={!!saving}><Icon name="folder" size={15} />Choose a file…</button>{/if}
    <button class="pill ghost" onclick={close}>Cancel</button>
  {/snippet}
</Modal>

<style>
  .current {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 14px;
    min-width: 0;
  }
  .now {
    width: 52px;
    height: 52px;
    flex: none;
    border-radius: 8px;
    overflow: hidden;
    display: grid;
    place-items: center;
    background: var(--surface);
    color: var(--text-3);
  }
  .now img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .what {
    min-width: 0;
    display: flex;
    flex-direction: column;
    font-size: 13px;
    color: var(--text-2);
  }
  .what strong {
    color: var(--text);
    font-size: 14.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .warn {
    margin: 0 0 14px;
    font-size: 13px;
    color: var(--warn);
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
  .grid {
    margin-top: 14px;
    max-height: min(420px, 50vh);
    overflow-y: auto;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(132px, 1fr));
    gap: 14px 12px;
    padding: 4px;
  }
  .opt {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
    text-align: left;
  }
  .opt:disabled {
    cursor: default;
  }
  .opt:disabled:not(.busy) {
    opacity: 0.5;
  }
  .pic {
    position: relative;
    aspect-ratio: 1;
    border-radius: 8px;
    overflow: hidden;
    background: var(--surface);
    margin-bottom: 4px;
    transition:
      transform 0.2s cubic-bezier(0.2, 0.8, 0.2, 1),
      box-shadow 0.2s;
  }
  .pic img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .opt:hover:not(:disabled) .pic {
    transform: scale(1.03);
    box-shadow: 0 0 0 3px var(--accent);
  }
  .busy .pic {
    box-shadow: 0 0 0 3px var(--accent);
    animation: pulse 1s ease-in-out infinite;
  }
  .match {
    position: absolute;
    left: 6px;
    top: 6px;
    font-size: 11px;
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 99px;
    color: white;
    background: color-mix(in srgb, var(--accent) 80%, black);
  }
  .label,
  .sub {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .label {
    font-size: 13px;
    font-weight: 600;
  }
  .sub {
    font-size: 12px;
    color: var(--text-2);
  }
  .ghost-opt {
    pointer-events: none;
  }
  .ghost-opt .pic,
  .ghost-opt i {
    animation: pulse 1.2s ease-in-out infinite;
  }
  .ghost-opt i {
    display: block;
    width: 70%;
    height: 9px;
    border-radius: 5px;
    background: var(--surface);
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 14px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: color-mix(in srgb, var(--warn) 14%, transparent);
    font-size: 13px;
  }
  .confirm span {
    flex: 1;
  }
  .small {
    height: 30px;
    padding: 0 12px;
    font-size: 12.5px;
  }
  .error {
    margin: 12px 0 0;
    color: var(--warn);
    font-size: 13px;
  }
  .note {
    margin-right: auto;
    align-self: center;
    font-size: 12.5px;
    color: var(--text-3);
  }
</style>

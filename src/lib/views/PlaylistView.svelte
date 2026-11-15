<script lang="ts" module>
  /** Undo of a removal: the removed ids put back into `current` (the list as it is now) at their
   *  old places, first place first (so together they land where they were), clamped to its end. */
  export function reinsert(current: string[], removed: { pos: number; id: string }[]): string[] {
    const out = [...current];
    for (const r of [...removed].sort((a, b) => a.pos - b.pos)) out.splice(Math.max(0, Math.min(r.pos, out.length)), 0, r.id);
    return out;
  }
</script>

<script lang="ts">
  import { artUrl, formatDuration, type Album, type Playlist, type Track } from "#lib/api.js";
  import Icon from "#lib/components/Icon.svelte";
  import Modal from "#lib/components/Modal.svelte";
  import TrackList from "#lib/components/TrackList.svelte";
  import { startExport } from "#lib/playlistTransfer.svelte.js";
  import { describe, evaluate } from "#lib/smart.js";
  import { app } from "#lib/store.svelte.js";

  let { id }: { id: string } = $props();
  const p = $derived(app.playlists.find((x) => x.id === id));
  // A normal playlist's rows: the songs the library still has, each with its real position in
  // `trackIds`. Ids the library doesn't have stay in the playlist (the file may come back), so a
  // row's number is not its position there: edits go through `pos`, never the row's index.
  const rows = $derived<{ t: Track; pos: number }[]>(
    !p || p.smart ? [] : p.trackIds.flatMap((id, pos) => {
      const t = app.trackById.get(id);
      return t ? [{ t, pos }] : [];
    }),
  );
  const tracks = $derived<Track[]>(!p ? [] : p.smart ? evaluate(p.smart, app.tracks, app.listening, app.marks, p.id) : rows.map((r) => r.t));
  const missing = $derived(!p || p.smart ? 0 : p.trackIds.length - rows.length);
  const covers = $derived([...new Set(tracks.map((t) => app.albumById.get(t.albumId)?.artSmall).filter(Boolean))].slice(0, 4) as string[]);
  const time = $derived(tracks.reduce((s, t) => s + t.duration, 0));
  /** The albums the playlist's songs come from (for "Shuffle albums": each played whole). */
  const albums = $derived([...new Set(tracks.map((t) => t.albumId))].map((id) => app.albumById.get(id)).filter((a): a is Album => !!a));

  let renaming = $state(false);
  let nameInput = $state("");

  function startRename() {
    nameInput = p!.name;
    renaming = true;
  }

  async function rename() {
    // Enter and Escape close the box first; the blur that follows must not save again (or at all, for Escape).
    if (!renaming) return;
    renaming = false;
    const name = nameInput.trim();
    try {
      if (p && name && name !== p.name) await app.savePlaylist({ ...p, name });
    } catch (e) {
      app.showToast(`Couldn’t rename the playlist: ${e}`);
    }
  }

  /** `from` and `to` are row numbers in what's shown; the splice uses their real places in the list.
   *  `picked`: several selected rows dragged together; they land as a block, in their order, before
   *  the drop row when dragged up and after it when dragged down (like one row does). */
  async function reorder(from: number, to: number, picked?: number[]) {
    if (!p || p.smart) return;
    const a = rows[from]?.pos;
    const b = rows[to]?.pos;
    if (a === undefined || b === undefined || a === b) return;
    let ids = [...p.trackIds];
    if (picked && picked.length > 1) {
      const moving = new Set(picked.flatMap((r) => (rows[r] ? [rows[r].pos] : [])));
      if (moving.has(b)) return;
      const block = ids.filter((_, k) => moving.has(k));
      // Ids the library doesn't have aren't rows: they stay where they are among the rest.
      const rest = ids.filter((_, k) => !moving.has(k));
      let at = b;
      for (const k of moving) if (k < b) at--;
      rest.splice(to > from ? at + 1 : at, 0, ...block);
      ids = rest;
    } else {
      const [moved] = ids.splice(a, 1);
      ids.splice(b, 0, moved);
    }
    try {
      await app.savePlaylist({ ...p, trackIds: ids });
    } catch (e) {
      app.showToast(`Couldn’t save the new order: ${e}`);
    }
  }

  /** Takes rows (numbers in what's shown) out of the playlist, by their real places; Undo puts the list back. */
  async function removeRows(picked: number[]) {
    if (!p || p.smart) return;
    const gone = new Set(picked.flatMap((r) => (rows[r] ? [rows[r].pos] : [])));
    if (!gone.size) return;
    const before = [...p.trackIds];
    const one = gone.size === 1 ? app.trackById.get(before[[...gone][0]]) : undefined;
    const ids = before.filter((_, k) => !gone.has(k));
    const removed = [...gone].sort((x, y) => x - y).map((pos) => ({ pos, id: before[pos] }));
    try {
      const saved = await app.savePlaylist({ ...p, trackIds: ids });
      app.showToast(one ? `Removed “${one.title}” from “${saved.name}”` : `Removed ${gone.size} songs from “${saved.name}”`, {
        label: "Undo",
        run: () => {
          app.toast = app.toastAction = null;
          // Into the list as it is now (songs added or moved since stay), each at its old place.
          const now = app.playlists.find((x) => x.id === saved.id);
          if (now) void app.savePlaylist({ ...now, trackIds: reinsert(now.trackIds, removed) }).catch((e) => app.showToast(`Couldn’t put them back: ${e}`));
        },
      });
    } catch (e) {
      app.showToast(`Couldn’t remove ${gone.size === 1 ? "the song" : "the songs"}: ${e}`);
    }
  }

  const removeAt = (i: number) => removeRows([i]);

  function deleteList() {
    if (!p) return;
    const gone = $state.snapshot(p) as Playlist;
    confirming = false;
    app.deletePlaylist(gone).then(
      () => app.showToast(`Deleted “${gone.name}”`, { label: "Undo", run: () => void app.savePlaylist(gone).catch((e) => app.showToast(`Couldn’t bring it back: ${e}`)) }, 8000),
      (e) => app.showToast(`Couldn’t delete the playlist: ${e}`),
    );
  }

  let confirming = $state(false);
</script>

{#if p}
  <div class="pl">
    <header class="hero">
      <div class="mosaic" class:single={covers.length < 4}>
        {#if covers.length >= 4}
          {#each covers as c}<img src={artUrl(c)} alt="" />{/each}
        {:else if covers.length}
          <img src={artUrl(covers[0])} alt="" />
        {:else}
          <div class="ph"><Icon name={p.smart ? "smart" : "playlist"} size={56} /></div>
        {/if}
      </div>
      <div class="info">
        <p class="kind"><Icon name={p.smart ? "smart" : "playlist"} size={14} />{p.smart ? "Smart playlist" : "Playlist"}</p>
        {#if renaming}
          <!-- svelte-ignore a11y_autofocus -->
          <input class="rename" bind:value={nameInput} autofocus onblur={rename} onkeydown={(e) => { if (e.key === "Enter") rename(); if (e.key === "Escape") { e.stopPropagation(); renaming = false; } }} aria-label="Playlist name" />
        {:else}
          <h1><button class="title" ondblclick={startRename} title="Double-click to rename">{p.name}</button></h1>
        {/if}
        {#if p.smart}<p class="rules">{describe(p.smart)}</p>{/if}
        <p class="facts">{tracks.length} {tracks.length === 1 ? "song" : "songs"}{time ? `, ${formatDuration(time)}` : ""}{#if missing}<span title="These songs aren’t in your library right now. They stay in the playlist in case the files come back."> · {missing} {missing === 1 ? "song" : "songs"} missing</span>{/if}</p>
        <div class="actions">
          <button class="pill" onclick={() => app.playTracks(tracks)} disabled={!tracks.length}><Icon name="play" size={15} />Play</button>
          <button class="pill ghost" onclick={() => app.shuffleTracks(tracks)} disabled={!tracks.length}><Icon name="shuffle" size={15} />Shuffle</button>
          {#if albums.length > 1}
            <button class="pill ghost" onclick={() => app.shuffleAlbums(albums)} title="The albums these songs are from, each played whole and in order, in a random order">
              <Icon name="album" size={15} />Shuffle albums
            </button>
          {/if}
          {#if p.smart}
            <button class="pill ghost" onclick={() => (app.smartEditing = structuredClone($state.snapshot(p)))}><Icon name="edit" size={15} />Edit rules</button>
          {/if}
          <button class="icon-btn" onclick={startRename} aria-label="Rename" title="Rename"><Icon name="edit" size={17} /></button>
          <button class="icon-btn" onclick={() => startExport(p)} aria-label="Export playlist" title="Export…"><Icon name="download" size={17} /></button>
          <button class="icon-btn" onclick={() => (confirming = true)} aria-label="Delete playlist" title="Delete playlist"><Icon name="trash" size={17} /></button>
        </div>
      </div>
    </header>

    <div class="tracks">
      {#if tracks.length}
        <TrackList
          {tracks}
          showArt
          showAlbum
          onreorder={p.smart ? undefined : reorder}
          onremove={p.smart ? undefined : removeRows}
          extraMenu={p.smart ? undefined : (_t, i) => [{ label: "Remove from this playlist", icon: "trash", action: () => removeAt(i) }]}
        />
        {#if !p.smart}<p class="hint">Drag songs to change the order.</p>{/if}
      {:else if p.smart}
        <p class="empty">No songs match these rules yet. <button class="link" onclick={() => (app.smartEditing = structuredClone($state.snapshot(p)))}>Edit the rules</button></p>
      {:else if missing}
        <p class="empty">None of this playlist’s songs are in your library right now. They stay here in case the files come back.</p>
      {:else}
        <p class="empty">This playlist is empty. Right-click any song or album and choose “Add to Playlist…”.</p>
      {/if}
    </div>
  </div>
  {#if confirming}
    <Modal title="Delete “{p.name}”?" onclose={() => (confirming = false)} width="420px">
      <p class="secondary">The playlist is removed from Cadence. Your music files aren’t touched.</p>
      {#snippet footer()}
        <button class="pill ghost" onclick={() => (confirming = false)}>Cancel</button>
        <button class="pill danger" onclick={deleteList}>Delete playlist</button>
      {/snippet}
    </Modal>
  {/if}
{:else}
  <div class="page secondary">This playlist no longer exists.</div>
{/if}

<style>
  .hero {
    display: flex;
    align-items: flex-end;
    gap: 32px;
    padding: 64px 44px 28px;
  }
  .mosaic {
    width: clamp(190px, 19vw, 250px);
    aspect-ratio: 1;
    flex: none;
    display: grid;
    grid-template-columns: 1fr 1fr;
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--surface);
    box-shadow: 0 24px 56px rgba(0, 0, 0, 0.5);
  }
  .mosaic.single {
    grid-template-columns: 1fr;
  }
  .mosaic img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .ph {
    display: grid;
    place-items: center;
    color: var(--accent);
    background: radial-gradient(circle at 30% 20%, color-mix(in srgb, var(--accent) 25%, transparent), transparent 70%);
  }
  .info {
    min-width: 0;
  }
  .kind {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--accent);
  }
  h1 {
    margin: 0;
  }
  .title,
  .rename {
    font-family: var(--display);
    font-size: clamp(28px, 3.2vw, 48px);
    font-weight: 750;
    font-variation-settings: "wdth" 88;
    letter-spacing: -0.03em;
    line-height: 1.05;
    text-align: left;
  }
  .rename {
    background: var(--hover);
    border: 1px solid var(--accent);
    border-radius: var(--r-sm);
    padding: 2px 8px;
    outline: none;
    width: min(600px, 100%);
  }
  .rules {
    margin: 10px 0 0;
    color: var(--text-2);
    max-width: 70ch;
  }
  .rules::first-letter {
    text-transform: uppercase;
  }
  .facts {
    margin: 8px 0 0;
    color: var(--text-3);
    font-size: 13px;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 20px;
  }
  .tracks {
    padding: 0 32px 24px;
  }
  .hint,
  .empty {
    color: var(--text-3);
    font-size: 13px;
    padding: 12px;
  }
  .empty {
    color: var(--text-2);
    font-size: 14px;
  }
  /* Phones: cover on top, text below. */
  @media (max-width: 720px) {
    .hero {
      display: flex !important;
      flex-direction: column;
      align-items: flex-start !important;
      gap: 18px !important;
      margin-bottom: 28px;
    }
    .hero .mosaic {
      width: min(76vw, 320px) !important;
      align-self: center;
    }
    h1 {
      font-size: 30px !important;
      max-width: none !important;
    }
    .actions {
      flex-wrap: wrap;
    }
  }
</style>

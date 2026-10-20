<script lang="ts">
  import { artUrl, formatDuration, type Track } from "$lib/api";
  import Icon from "$lib/components/Icon.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import TrackList from "$lib/components/TrackList.svelte";
  import { describe, evaluate } from "$lib/smart";
  import { app, shuffled } from "$lib/store.svelte";

  let { id }: { id: string } = $props();
  const p = $derived(app.playlists.find((x) => x.id === id));
  const tracks = $derived<Track[]>(
    !p ? [] : p.smart ? evaluate(p.smart, app.tracks, app.listening, p.id) : p.trackIds.map((t) => app.trackById.get(t)).filter((t): t is Track => !!t),
  );
  const covers = $derived([...new Set(tracks.map((t) => app.albumById.get(t.albumId)?.artSmall).filter(Boolean))].slice(0, 4) as string[]);
  const time = $derived(tracks.reduce((s, t) => s + t.duration, 0));

  let renaming = $state(false);
  let nameInput = $state("");

  function startRename() {
    nameInput = p!.name;
    renaming = true;
  }

  async function rename() {
    renaming = false;
    const name = nameInput.trim();
    if (p && name && name !== p.name) await app.savePlaylist({ ...p, name });
  }

  async function reorder(from: number, to: number) {
    if (!p) return;
    const ids = [...p.trackIds];
    const [moved] = ids.splice(from, 1);
    ids.splice(to, 0, moved);
    await app.savePlaylist({ ...p, trackIds: ids });
  }

  async function removeAt(i: number) {
    if (!p) return;
    const ids = [...p.trackIds];
    ids.splice(i, 1);
    await app.savePlaylist({ ...p, trackIds: ids });
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
          <input class="rename" bind:value={nameInput} autofocus onblur={rename} onkeydown={(e) => { if (e.key === "Enter") rename(); if (e.key === "Escape") renaming = false; }} aria-label="Playlist name" />
        {:else}
          <h1><button class="title" ondblclick={startRename} title="Double-click to rename">{p.name}</button></h1>
        {/if}
        {#if p.smart}<p class="rules">{describe(p.smart)}</p>{/if}
        <p class="facts">{tracks.length} {tracks.length === 1 ? "song" : "songs"}{time ? `, ${formatDuration(time)}` : ""}</p>
        <div class="actions">
          <button class="pill" onclick={() => app.playTracks(tracks)} disabled={!tracks.length}><Icon name="play" size={15} />Play</button>
          <button class="pill ghost" onclick={() => app.playTracks(shuffled(tracks))} disabled={!tracks.length}><Icon name="shuffle" size={15} />Shuffle</button>
          {#if p.smart}
            <button class="pill ghost" onclick={() => (app.smartEditing = structuredClone($state.snapshot(p)))}><Icon name="edit" size={15} />Edit rules</button>
          {:else}
            <button class="icon-btn" onclick={startRename} aria-label="Rename" title="Rename"><Icon name="edit" size={17} /></button>
          {/if}
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
          extraMenu={p.smart ? undefined : (_t, i) => [{ label: "Remove from this playlist", icon: "trash", action: () => removeAt(i) }]}
        />
        {#if !p.smart}<p class="hint">Drag songs to change the order.</p>{/if}
      {:else if p.smart}
        <p class="empty">No songs match these rules yet. <button class="link" onclick={() => (app.smartEditing = structuredClone($state.snapshot(p)))}>Edit the rules</button></p>
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
        <button class="pill danger" onclick={() => { confirming = false; app.deletePlaylist(p); }}>Delete playlist</button>
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

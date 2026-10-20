<script lang="ts">
  import AlbumCard from "$lib/components/AlbumCard.svelte";
  import TrackList from "$lib/components/TrackList.svelte";
  import { app } from "$lib/store.svelte";

  let { query }: { query: string } = $props();

  // Case- and accent-insensitive matching, so "pokemon" finds "Pokémon".
  const fold = (s: string) => s.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase();
  const q = $derived(fold(query.trim()));
  const albums = $derived(app.albums.filter((a) => fold(a.title).includes(q) || fold(a.artist).includes(q)).slice(0, 24));
  const songs = $derived(
    app.tracks.filter((t) => fold(t.title).includes(q) || fold(t.artist).includes(q) || fold(t.album).includes(q)).slice(0, 200),
  );
  const artists = $derived(app.artists.filter((a) => fold(a.name).includes(q)).slice(0, 12));
</script>

<div class="page">
  <!-- Phones have no sidebar search box, so the page has its own. -->
  <input class="field page-search" type="search" placeholder="Search your music" value={query} aria-label="Search your music"
    oninput={(e) => (app.view = { name: "search", query: e.currentTarget.value })} />
  {#if query.trim()}<h1 class="page-title">“{query}”</h1>{/if}

  {#if q && !albums.length && !songs.length && !artists.length}
    <p class="secondary empty">Nothing in your library matches. Try part of a title, an artist or an album name.</p>
  {/if}

  {#if artists.length}
    <h2 class="section-title">Artists</h2>
    <div class="chips">
      {#each artists as a (a.name)}
        <button class="chip" onclick={() => app.go({ name: "artist", artist: a.name })}>{a.name}</button>
      {/each}
    </div>
  {/if}

  {#if albums.length}
    <h2 class="section-title">Albums</h2>
    <div class="grid">
      {#each albums as album (album.id)}
        <AlbumCard {album} />
      {/each}
    </div>
  {/if}

  {#if songs.length}
    <h2 class="section-title">Songs</h2>
    <TrackList tracks={songs} showArt showAlbum />
  {/if}
</div>

<style>
  .empty {
    margin-top: 14px;
  }
  .page-search {
    display: none;
    width: 100%;
    font-size: 16px;
    padding: 10px 14px;
    border-radius: 12px;
    margin-bottom: 18px;
  }
  @media (max-width: 720px) {
    .page-search {
      display: block;
    }
  }
  .section-title {
    margin: 34px 0 16px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: 28px 22px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .chip {
    height: 32px;
    padding: 0 16px;
    border-radius: 999px;
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    font-weight: 500;
  }
  .chip:hover {
    background: var(--active);
  }
  /* Phones: two albums per row. */
  @media (max-width: 720px) {
    .grid {
      grid-template-columns: repeat(2, minmax(0, 1fr)) !important;
      gap: 20px 14px !important;
    }
  }
</style>

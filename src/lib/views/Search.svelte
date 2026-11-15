<script lang="ts" module>
  import type { Album, Track } from "#lib/api.js";
  import type { Artist } from "#lib/store.svelte.js";

  // Case- and accent-insensitive matching, so "pokemon" finds "Pokémon".
  const fold = (s: string) => s.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase();

  // The library's names, folded once per library (the lists are replaced whole when it changes,
  // so the list itself is the key), not on every key press.
  const cache = new WeakMap<object, string[][]>();
  function folded<T>(list: T[], fields: (x: T) => string[]): string[][] {
    let f = cache.get(list);
    if (!f) {
      f = list.map((x) => fields(x).map(fold));
      cache.set(list, f);
    }
    return f;
  }
  function matching<T>(list: T[], fields: (x: T) => string[], q: string, max: number): T[] {
    const f = folded(list, fields);
    const out: T[] = [];
    for (let i = 0; i < list.length && out.length < max; i++) if (f[i].some((s) => s.includes(q))) out.push(list[i]);
    return out;
  }
  const albumFields = (a: Album) => [a.title, a.artist];
  const trackFields = (t: Track) => [t.title, t.subtitle ?? "", t.artist, t.album];
  const artistFields = (a: Artist) => [a.name];
</script>

<script lang="ts">
  import AlbumCard from "#lib/components/AlbumCard.svelte";
  import TrackList from "#lib/components/TrackList.svelte";
  import { app } from "#lib/store.svelte.js";

  let { query }: { query: string } = $props();

  // What's searched for: the typed text a moment (120 ms) after the last key, so a fast typist
  // doesn't search the library for every letter. Coming back to the page shows its results at once.
  const typed = () => fold(query.trim());
  let q = $state(typed());
  $effect(() => {
    const next = typed();
    if (next === q) return;
    // Emptied: straight away.
    if (!next) {
      q = "";
      return;
    }
    const timer = setTimeout(() => (q = next), 120);
    return () => clearTimeout(timer);
  });
  // Nothing is listed until something is typed (an empty search would match the whole library).
  const albums = $derived(!q ? [] : matching(app.albums, albumFields, q, 24));
  const songs = $derived(!q ? [] : matching(app.tracks, trackFields, q, 200));
  const artists = $derived(!q ? [] : matching(app.artists, artistFields, q, 12));

  function onInput(value: string) {
    app.search = value;
    app.view = { name: "search", query: value };
  }

  /** Escape: empty the box and go back to the page before the search. */
  function onKey(e: KeyboardEvent) {
    if (e.key !== "Escape") return;
    e.preventDefault();
    app.search = "";
    if (query.trim()) app.back();
    else (e.currentTarget as HTMLInputElement).blur();
  }
</script>

<div class="page">
  <!-- Phones have no sidebar search box, so the page has its own. -->
  <input class="field page-search" type="search" placeholder="Search your music" value={query} aria-label="Search your music"
    oninput={(e) => onInput(e.currentTarget.value)} onkeydown={onKey} />
  {#if query.trim()}<h1 class="page-title">“{query}”</h1>{/if}

  {#if !q}
    <p class="secondary empty">Find songs, albums and artists in your library by any part of their name.</p>
  {:else if !albums.length && !songs.length && !artists.length}
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
  /* Android wider than a phone has no sidebar either. */
  :global(html.phone) .page-search {
    display: block;
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

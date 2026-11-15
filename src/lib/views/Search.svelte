<script lang="ts" module>
  import type { Album, Track } from "#lib/api.js";
  import type { Artist } from "#lib/store.svelte.js";
  import { fold, matching } from "#lib/searchIndex.js";
  import { extraMatch } from "#lib/searchKeys.js";

  // Case- and accent-insensitive matching ("pokemon" finds "Pokémon") and its cached index live in
  // searchIndex.ts: the command palette uses them too.
  const albumFields = (a: Album) => [a.title, a.artist];
  const trackFields = (t: Track) => [t.title, t.subtitle ?? "", t.artist, t.album];
  const artistFields = (a: Artist) => [a.name];
</script>

<script lang="ts">
  import AlbumCard from "#lib/components/AlbumCard.svelte";
  import TrackList from "#lib/components/TrackList.svelte";
  import { api, formatTime, type LyricHit } from "#lib/api.js";
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
  // After what the names match: what the artists' other names and the titles' readings find
  // ("yonezu" finds 米津玄師, "tokyo" finds とうきょう).
  const extra = $derived(q ? extraMatch(app.searchKeys, q) : null);
  const albums = $derived(!q ? [] : matching(app.albums, albumFields, q, 24, extra?.album));
  const songs = $derived(!q ? [] : matching(app.tracks, trackFields, q, 200, extra?.track));
  const artists = $derived(!q ? [] : matching(app.artists, artistFields, q, 12, extra?.artist));

  // Lyrics: songs with a line matching what's typed (asked of the app once per query, after the
  // same pause; the first time, it makes its index in the background and says when it's ready).
  let lyricHits = $state<LyricHit[]>([]);
  let lyricPending = $state(false);
  $effect(() => {
    const text = q;
    void app.lyricIndexAt; // the index is ready, or changed: ask again
    if (text.replace(/\s/g, "").length < 2) {
      lyricHits = [];
      lyricPending = false;
      return;
    }
    let current = true;
    api
      .lyricSearch(text, 12)
      .then((r) => {
        if (!current) return;
        lyricPending = r?.ready === false;
        lyricHits = Array.isArray(r?.hits) ? r.hits.filter((h) => h && app.trackById.has(h.trackId) && typeof h.line === "string") : [];
      })
      .catch(() => {
        if (current) lyricHits = [];
      });
    return () => {
      current = false;
    };
  });

  /** Plays the song (with the rest of its album after it) from the line found, with this song's
   *  timing nudge from the lyrics view. */
  async function playLine(h: LyricHit) {
    const t = app.trackById.get(h.trackId);
    if (!t) return;
    const album = app.albumById.get(t.albumId);
    const list = album ? app.albumTracks(album) : [];
    const at = list.findIndex((x) => x.id === t.id);
    const tracks = at >= 0 ? list : [t];
    await api.play(tracks.map((x) => x.id), Math.max(at, 0), true);
    if (h.time != null && h.time > 0) {
      let nudge = 0;
      try {
        const v = JSON.parse(localStorage.getItem("cadence-lyric-offsets") ?? "{}")[t.id];
        if (typeof v === "number" && isFinite(v)) nudge = v;
      } catch {
        /* no nudges kept */
      }
      app.seek(Math.max(0, h.time + nudge));
    }
  }

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
  <input class="field page-search" data-global-search="page" type="search" placeholder="Search your music" value={query} aria-label="Search your music"
    oninput={(e) => onInput(e.currentTarget.value)} onkeydown={onKey} />
  {#if query.trim()}<h1 class="page-title">“{query}”</h1>{/if}

  {#if !q}
    <p class="secondary empty">Find songs, albums and artists in your library by any part of their name.</p>
  {:else if !albums.length && !songs.length && !artists.length && !lyricHits.length}
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
    <TrackList tracks={songs} showArt showAlbum identity={q} />
  {/if}

  {#if lyricHits.length}
    <h2 class="section-title">Lyrics</h2>
    <ul class="lyric-hits">
      {#each lyricHits as h (h.trackId)}
        {@const t = app.trackById.get(h.trackId)}
        {#if t}
          <li>
            <button class="lyric-hit" onclick={() => playLine(h)} title={h.time != null ? `Play from ${formatTime(h.time)}` : "Play"}
              aria-label={`Play ${t.title} from the line: ${h.line}`}>
              <span class="lh-song"><span class="lh-title">{t.title}</span><span class="lh-artist">{t.artist}</span></span>
              <span class="lh-line">{h.line}</span>
              {#if h.time != null}<span class="lh-time">{formatTime(h.time)}</span>{/if}
            </button>
          </li>
        {/if}
      {/each}
    </ul>
  {:else if lyricPending}
    <p class="secondary lyric-wait">Reading your saved lyrics once, so songs can be found by a line. They’ll show here in a moment.</p>
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
  /* Songs found by a line of their lyrics: the song, then the line, then where it's sung. */
  .lyric-hits {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .lyric-hit {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 9px 12px;
    border-radius: var(--r-md);
    text-align: left;
  }
  .lyric-hit:hover {
    background: var(--hover);
  }
  .lh-song {
    flex: 0 0 min(34%, 260px);
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .lh-title,
  .lh-artist,
  .lh-line {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .lh-title {
    font-weight: 600;
  }
  .lh-artist {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .lh-line {
    flex: 1;
    min-width: 0;
    font-style: italic;
    color: var(--text-2);
  }
  .lh-time {
    flex: none;
    font-size: 12.5px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .lyric-wait {
    margin-top: 34px;
  }
  /* Phones: two albums per row. */
  @media (max-width: 720px) {
    .grid {
      grid-template-columns: repeat(2, minmax(0, 1fr)) !important;
      gap: 20px 14px !important;
    }
  }
</style>

<script lang="ts" module>
  import { untrack } from "svelte";
  import { albumFields, artistFields, fold, matchingNames, matchingOthers, trackFields, type Extra } from "#lib/searchIndex.js";
  import { extraMatch } from "#lib/searchKeys.js";

  // Case- and accent-insensitive matching ("pokemon" finds "Pokémon") and its cached index live in
  // searchIndex.ts: the command palette uses them too. The scans read the store's lists without
  // subscribing to every song in them (the list itself, replaced whole on a change, is what counts).
  const find = <T,>(list: T[], fields: (x: T) => string[], q: string, max: number) => untrack(() => matchingNames(list, fields, q, max));
  const others = <T,>(list: T[], fields: (x: T) => string[], q: string, max: number, extra: Extra<T>) =>
    untrack(() => matchingOthers(list, fields, q, max, extra));
  /** The names' matches, then those found by other names; the same array when there are none of
   *  those (the search keys arriving then changes nothing on screen). */
  const joined = <T,>(named: T[], more: T[]) => (more.length ? [...named, ...more] : named);
  const NONE: never[] = [];
</script>

<script lang="ts">
  import AlbumCard from "#lib/components/AlbumCard.svelte";
  import TrackList from "#lib/components/TrackList.svelte";
  import { api, formatTime, type LyricHit } from "#lib/api.js";
  import { app } from "#lib/store.svelte.js";
  import { isFolded } from "#lib/searchIndex.js";
  import { warmSearch } from "#lib/searchWarm.js";
  import { onMount } from "svelte";

  let { query }: { query: string } = $props();

  /** How many of each are found: the songs list shows the rows on screen (it's virtual). */
  const SONGS = 200;
  const ALBUMS = 24;
  const ARTISTS = 12;
  /** The albums shown before "Show all": about two rows of cards. */
  const FIRST_ALBUMS = 12;

  // What's searched for: the typed text a moment (120 ms) after the last key, so a fast typist
  // doesn't search the library for every letter. Coming back to the page shows its results at once.
  // (Opened by the first key with the index not made yet: that key waits the same pause, while
  // the index is made, instead of making it on the way to the page.)
  const typed = () => fold(query.trim());
  let q = $state(isFolded(app.tracks) ? typed() : "");
  /** All the albums found shown (at first, the rows that fit on screen). */
  let allAlbums = $state(false);
  const setQ = (next: string) => {
    q = next;
    allAlbums = false;
  };
  $effect(() => {
    const next = typed();
    if (next === q) return;
    // Emptied: straight away.
    if (!next) {
      setQ("");
      return;
    }
    const timer = setTimeout(() => setQ(next), 120);
    return () => clearTimeout(timer);
  });
  // Nothing is listed until something is typed (an empty search would match the whole library).
  // After what the names match: what the artists' other names and the titles' readings find
  // ("yonezu" finds 米津玄師, "tokyo" finds とうきょう).
  const extra = $derived(q ? extraMatch(app.searchKeys, q) : null);
  const albumsNamed = $derived(q ? find(app.albums, albumFields, q, ALBUMS) : NONE);
  const songsNamed = $derived(q ? find(app.tracks, trackFields, q, SONGS) : NONE);
  const artistsNamed = $derived(q ? find(app.artists, artistFields, q, ARTISTS) : NONE);
  const albums = $derived(joined(albumsNamed, extra ? others(app.albums, albumFields, q, ALBUMS - albumsNamed.length, extra.album) : NONE));
  const songs = $derived(joined(songsNamed, extra ? others(app.tracks, trackFields, q, SONGS - songsNamed.length, extra.track) : NONE));
  const artists = $derived(joined(artistsNamed, extra ? others(app.artists, artistFields, q, ARTISTS - artistsNamed.length, extra.artist) : NONE));
  const shownAlbums = $derived(allAlbums ? albums : albums.slice(0, FIRST_ALBUMS));

  // Lyrics: songs with a line matching what's typed (asked of the app once per query, after the
  // same pause; the first time, it makes its index in the background and says when it's ready).
  let lyricHits = $state<LyricHit[]>([]);
  let lyricPending = $state(false);
  /** The text the hits shown were found for: another query's hits never stay under a new one. */
  let hitsFor = "";
  $effect(() => {
    const text = q;
    void app.lyricIndexAt; // the index is ready, or changed: ask again
    if (text !== hitsFor) {
      hitsFor = text;
      lyricHits = [];
    }
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
    let from = 0;
    if (h.time != null && h.time > 0) {
      let nudge = 0;
      try {
        const v = JSON.parse(localStorage.getItem("cadence-lyric-offsets") ?? "{}")[t.id];
        if (typeof v === "number" && isFinite(v)) nudge = v;
      } catch {
        /* no nudges kept */
      }
      from = Math.max(0, h.time + nudge);
    }
    // The seek waits for the song to start (sent at once it could land on the song before).
    try {
      await app.playFrom(tracks.map((x) => x.id), Math.max(at, 0), from);
    } catch (e) {
      app.showToast(`Couldn't play it: ${e}`);
    }
  }

  // The other names and readings ("yonezu" finds 米津玄師): asked for when Search first opens.
  onMount(() => {
    app.wantSearchKeys();
    warmSearch();
  });

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
      {#each shownAlbums as album (album.id)}
        <AlbumCard {album} />
      {/each}
    </div>
    {#if shownAlbums.length < albums.length}
      <button class="pill ghost more" onclick={() => (allAlbums = true)}>Show more albums</button>
    {/if}
  {/if}

  {#if songs.length}
    <h2 class="section-title">Songs</h2>
    <TrackList tracks={songs} showArt showAlbum identity={q} />
  {/if}

  {#if lyricHits.length}
    <h2 class="section-title">Lyrics</h2>
    <ul class="lyric-hits" class:stale={typed() !== q}>
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
  .more {
    margin-top: 12px;
  }
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
  /* The query changed and these are the last one's (for the moment before the new search). */
  .lyric-hits.stale {
    opacity: 0.45;
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

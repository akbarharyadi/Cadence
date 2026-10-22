<script lang="ts">
  import { artUrl, formatDuration } from "$lib/api";
  import AlbumCard from "$lib/components/AlbumCard.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import TrackList from "$lib/components/TrackList.svelte";
  import { app, shuffled } from "$lib/store.svelte";

  let { name }: { name: string } = $props();
  const artist = $derived(app.artists.find((a) => a.name.toLowerCase() === name.toLowerCase()));
  const albums = $derived([...(artist?.albums ?? [])].sort((a, b) => (b.year ?? 0) - (a.year ?? 0)));
  const songs = $derived(albums.flatMap((a) => app.albumTracks(a)));
  const portrait = $derived(albums[0]);
  const picture = $derived(artist ? app.artistPicture(artist, "large") : null);
  const time = $derived(songs.reduce((s, t) => s + t.duration, 0));
</script>

{#if artist}
  <div class="artist-page">
    <header class="hero">
      <button class="portrait" style="--glow: {portrait?.accent ?? 'transparent'}" onclick={() => (app.artistEditing = artist.name)} aria-label="Change the picture for {artist.name}" title="Change picture">
        {#key picture}
          {#if picture}<img src={artUrl(picture)} alt="" />{:else}<Icon name="artist" size={64} />{/if}
        {/key}
        <span class="edit"><Icon name="edit" size={20} /><span>Change</span></span>
      </button>
      <div class="info">
        <h1>{artist.name}</h1>
        <p class="facts">{albums.length} {albums.length === 1 ? "album" : "albums"}, {songs.length} songs, {formatDuration(time)}</p>
        <div class="actions">
          <button class="pill" onclick={() => app.playTracks(songs)}><Icon name="play" size={15} />Play</button>
          <button class="pill ghost" onclick={() => app.playTracks(shuffled(songs))}><Icon name="shuffle" size={15} />Shuffle</button>
        </div>
      </div>
    </header>

    <section>
      <h2 class="section-title">Albums</h2>
      <div class="grid">
        {#each albums as album (album.id)}<AlbumCard {album} />{/each}
      </div>
    </section>

    <section>
      <h2 class="section-title">Songs</h2>
      <TrackList tracks={songs} showArt showAlbum />
    </section>
  </div>
{:else}
  <div class="page secondary">No music by {name} in your library.</div>
{/if}

<style>
  .artist-page {
    padding: 64px 44px 40px;
  }
  .hero {
    display: flex;
    align-items: center;
    gap: 32px;
    margin-bottom: 40px;
  }
  .portrait {
    width: 190px;
    height: 190px;
    flex: none;
    border-radius: 50%;
    overflow: hidden;
    background: var(--surface);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow:
      0 20px 50px rgba(0, 0, 0, 0.5),
      0 6px 40px color-mix(in srgb, var(--glow) 30%, transparent);
  }
  .portrait {
    position: relative;
    padding: 0;
  }
  .portrait img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    animation: picture-in 0.4s ease-out;
  }
  @keyframes picture-in {
    from {
      opacity: 0;
    }
  }
  /* "Change" appears over the picture on hover (always reachable by keyboard). */
  .edit {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    font-size: 13px;
    font-weight: 600;
    color: white;
    background: rgba(0, 0, 0, 0.5);
    opacity: 0;
    transition: opacity 0.18s;
  }
  .portrait:hover .edit,
  .portrait:focus-visible .edit {
    opacity: 1;
  }
  @media (hover: none) {
    .edit {
      inset: auto 0 0;
      height: 34%;
      flex-direction: row;
      opacity: 1;
      background: linear-gradient(transparent, rgba(0, 0, 0, 0.6));
    }
    .edit span {
      display: none;
    }
  }
  h1 {
    margin: 0;
    font-family: var(--display);
    font-size: clamp(32px, 4vw, 56px);
    font-weight: 750;
    font-variation-settings: "wdth" 88;
    letter-spacing: -0.03em;
    line-height: 1;
  }
  .facts {
    margin: 10px 0 0;
    color: var(--text-2);
  }
  .actions {
    display: flex;
    gap: 10px;
    margin-top: 20px;
  }
  section {
    margin-top: 38px;
  }
  section .section-title {
    margin-bottom: 18px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 30px 24px;
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
    .hero .portrait {
      width: min(56vw, 220px) !important;
      height: auto !important;
      aspect-ratio: 1;
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
  /* Phones: two albums per row. */
  @media (max-width: 720px) {
    .grid {
      grid-template-columns: repeat(2, minmax(0, 1fr)) !important;
      gap: 20px 14px !important;
    }
  }
</style>

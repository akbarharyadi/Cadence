<script lang="ts">
  import { artUrl, formatDuration } from "$lib/api";
  import Icon from "$lib/components/Icon.svelte";
  import QualityBadge from "$lib/components/QualityBadge.svelte";
  import TrackList from "$lib/components/TrackList.svelte";
  import { app } from "$lib/store.svelte";

  let { id }: { id: string } = $props();
  const album = $derived(app.albumById.get(id));
  const tracks = $derived(album ? app.albumTracks(album) : []);
  const playing = $derived(app.currentAlbum?.id === id);
  const facts = $derived(album ? [album.genre, album.year?.toString()].filter(Boolean) : []);
</script>

{#if album}
  <div class="album" style="--album-accent: {album.accent ?? 'var(--accent)'}">
    <header class="hero">
      <div class="cover">
        {#if album.art}<img src={artUrl(album.art)} alt="" />{:else}<div class="ph"><Icon name="album" size={64} /></div>{/if}
      </div>
      <div class="info">
        <h1>{album.title}</h1>
        <button class="artist" onclick={() => app.go({ name: "artist", artist: app.artistOf(album.artist) })}>{album.artist}</button>
        <div class="facts">
          {#each facts as f}<span>{f}</span>{/each}
          <QualityBadge quality={album.quality} detail={album.format} />
        </div>
        <div class="actions">
          {#if playing}
            <button class="pill" onclick={() => app.togglePlay()}>
              <Icon name={app.status?.playing ? "pause" : "play"} size={15} />{app.status?.playing ? "Pause" : "Resume"}
            </button>
          {:else}
            <button class="pill" onclick={() => app.playAlbum(album)}><Icon name="play" size={15} />Play</button>
          {/if}
          <button class="pill ghost" onclick={() => app.playAlbum(album, true)}><Icon name="shuffle" size={15} />Shuffle</button>
          <button class="icon-btn more" aria-label="More options" title="More options" onclick={(e) => app.albumMenu(e, album)}><Icon name="more" size={18} /></button>
        </div>
      </div>
    </header>

    <div class="tracks">
      <TrackList {tracks} numbered hideArtist={album.artist} />
      <p class="foot">{tracks.length} {tracks.length === 1 ? "song" : "songs"}, {formatDuration(album.duration)}</p>
    </div>
  </div>
{:else}
  <div class="page secondary">This album is no longer in your library.</div>
{/if}

<style>
  .hero {
    display: flex;
    align-items: flex-end;
    gap: 34px;
    padding: 64px 44px 30px;
  }
  .cover {
    width: clamp(200px, 21vw, 280px);
    aspect-ratio: 1;
    flex: none;
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--surface);
    box-shadow:
      0 26px 60px rgba(0, 0, 0, 0.5),
      0 8px 50px color-mix(in srgb, var(--album-accent) 22%, transparent);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .ph {
    height: 100%;
    display: grid;
    place-items: center;
    color: var(--text-3);
  }
  .info {
    min-width: 0;
    padding-bottom: 4px;
  }
  h1 {
    margin: 0;
    font-family: var(--display);
    font-size: clamp(26px, 2.8vw, 42px);
    font-weight: 750;
    font-variation-settings: "wdth" 88;
    line-height: 1.05;
    letter-spacing: -0.025em;
    max-width: 24ch;
    overflow-wrap: anywhere;
  }
  .artist {
    margin-top: 8px;
    font-size: 19px;
    font-weight: 500;
    color: var(--album-accent);
    text-align: left;
  }
  .artist:hover {
    text-decoration: underline;
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 16px;
    margin-top: 12px;
    font-size: 13px;
    color: var(--text-3);
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
  .foot {
    color: var(--text-3);
    font-size: 13px;
    padding: 12px 12px 0;
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
    .hero .cover {
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

<script lang="ts">
  import { api, artUrl, type Track } from "$lib/api";
  import Icon from "$lib/components/Icon.svelte";
  import TrackList from "$lib/components/TrackList.svelte";
  import { app } from "$lib/store.svelte";

  let { id }: { id: string } = $props();
  const seed = $derived(app.trackById.get(id));
  const album = $derived(seed ? app.albumById.get(seed.albumId) : undefined);

  let ids = $state<string[] | null>(null);
  $effect(() => {
    const current = id;
    // Look again when analysis finishes, in case this song wasn't analysed yet.
    void app.sonic?.analyzed;
    api.similarTracks(current, 40).then((r) => {
      if (current === id) ids = r;
    });
  });
  const tracks = $derived((ids ?? []).map((t) => app.trackById.get(t)).filter((t): t is Track => !!t));
</script>

{#if seed}
  <div class="similar">
    <header class="hero">
      <div class="cover">
        {#if album?.art}<img src={artUrl(album.art)} alt="" />{:else}<Icon name="song" size={56} />{/if}
        <span class="badge"><Icon name="wave" size={16} /></span>
      </div>
      <div class="info">
        <p class="kind">Sounds like</p>
        <h1>{seed.title}</h1>
        <p class="by">{seed.artist}</p>
        <p class="note">Picked by how the music sounds: tempo, timbre, loudness and harmony. Analysed on this computer.</p>
        <div class="actions">
          <button class="pill" onclick={() => app.soundRadio(seed)}><Icon name="radio" size={15} />Start radio</button>
          <button class="pill ghost" onclick={() => app.playTracks([seed, ...tracks])} disabled={!tracks.length}><Icon name="play" size={15} />Play these</button>
        </div>
      </div>
    </header>

    {#if tracks.length}
      <TrackList {tracks} showArt showAlbum />
    {:else if ids}
      <div class="empty">
        <Icon name="wave" size={28} />
        {#if app.sonic?.running}
          <p>Cadence is still listening to your songs ({app.sonic.analyzed} of {app.sonic.total}). Similar songs appear here when it's done.</p>
        {:else}
          <p>No other songs to compare yet. Add more music and Cadence will find the ones that sound alike.</p>
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .similar {
    padding: 64px 44px 40px;
    max-width: 1500px;
  }
  .hero {
    display: flex;
    align-items: flex-end;
    gap: 32px;
    margin-bottom: 32px;
  }
  .cover {
    position: relative;
    width: clamp(170px, 17vw, 220px);
    aspect-ratio: 1;
    flex: none;
    border-radius: var(--r-lg);
    background: var(--surface);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow: 0 24px 50px rgba(0, 0, 0, 0.45);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: inherit;
  }
  .badge {
    position: absolute;
    right: -10px;
    bottom: -10px;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: #0d0e17;
    box-shadow: 0 6px 16px rgba(0, 0, 0, 0.4);
  }
  .info {
    min-width: 0;
  }
  .kind {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--accent);
  }
  h1 {
    margin: 0;
    font-family: var(--display);
    font-size: clamp(26px, 2.8vw, 42px);
    font-weight: 750;
    font-variation-settings: "wdth" 88;
    letter-spacing: -0.03em;
    line-height: 1.04;
    overflow-wrap: anywhere;
  }
  .by {
    margin: 8px 0 0;
    font-size: 17px;
    color: var(--text-2);
  }
  .note {
    margin: 12px 0 0;
    max-width: 60ch;
    font-size: 13px;
    color: var(--text-3);
  }
  .actions {
    display: flex;
    gap: 10px;
    margin-top: 20px;
  }
  .empty {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 22px 24px;
    border-radius: var(--r-lg);
    background: var(--surface);
    color: var(--text-2);
    max-width: 640px;
  }
  .empty p {
    margin: 0;
  }
</style>

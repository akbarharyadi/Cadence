<script lang="ts">
  import Icon from "#lib/components/Icon.svelte";
  import TrackList from "#lib/components/TrackList.svelte";
  import { app } from "#lib/store.svelte.js";

  // One collator for the whole sort (the same order as `localeCompare`, several times faster on
  // 20,000 songs: `localeCompare` sets up its comparison again on every call).
  const collator = new Intl.Collator();
  const songs = $derived([...app.tracks].sort((a, b) => collator.compare(a.title, b.title)));
  const skipped = $derived(songs.filter((t) => app.isSkipped(t)).length);
</script>

<div class="page">
  <header>
    <div>
      <h1 class="page-title">Songs</h1>
      <p class="secondary count">{songs.length} songs{#if skipped}, {skipped} skipped{/if}</p>
    </div>
    <div class="actions">
      <button class="pill ghost" onclick={() => app.go({ name: "favorites" })}><Icon name="heart" size={15} />Favorites</button>
      <button class="pill" onclick={() => app.playTracks(songs)}><Icon name="play" size={15} />Play</button>
      <button class="pill ghost" onclick={() => app.shuffleTracks(songs)}><Icon name="shuffle" size={15} />Shuffle</button>
    </div>
  </header>
  <TrackList tracks={songs} showArt showAlbum />
</div>

<style>
  header {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 20px;
    margin-bottom: 22px;
  }
  .count {
    margin: 6px 0 0;
    font-size: 13px;
  }
  .actions {
    display: flex;
    gap: 10px;
  }
</style>

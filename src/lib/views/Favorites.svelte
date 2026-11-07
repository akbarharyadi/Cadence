<script lang="ts">
  import Icon from "#lib/components/Icon.svelte";
  import TrackList from "#lib/components/TrackList.svelte";
  import { app, shuffled } from "#lib/store.svelte.js";

  const songs = $derived(app.favorites);
</script>

<div class="page">
  <header>
    <div>
      <h1 class="page-title">Favorites</h1>
      <p class="secondary count">{songs.length} {songs.length === 1 ? "song" : "songs"}, best rated first</p>
    </div>
    {#if songs.length}
      <div class="actions">
        <button class="pill" onclick={() => app.playTracks(songs)}><Icon name="play" size={15} />Play</button>
        <button class="pill ghost" onclick={() => app.playTracks(shuffled(songs))}><Icon name="shuffle" size={15} />Shuffle</button>
      </div>
    {/if}
  </header>
  {#if songs.length}
    <TrackList tracks={songs} showArt showAlbum />
  {:else}
    <p class="secondary empty">
      Songs you heart <Icon name="heart" size={14} /> show up here. Use the heart in the player, next to a song, or in its menu.
    </p>
  {/if}
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
  .empty {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
</style>

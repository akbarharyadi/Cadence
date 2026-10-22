<script lang="ts">
  import { artUrl } from "$lib/api";
  import Icon from "$lib/components/Icon.svelte";
  import { app } from "$lib/store.svelte";
</script>

<div class="page">
  <h1 class="page-title">Artists</h1>
  <p class="secondary count">{app.artists.length} {app.artists.length === 1 ? "artist" : "artists"}</p>
  <div class="grid">
    {#each app.artists as artist (artist.name)}
      {@const picture = app.artistPicture(artist, "small")}
      <button
        class="artist"
        onclick={() => app.go({ name: "artist", artist: artist.name })}
        oncontextmenu={(e) => app.openMenu(e, [
          { label: "Go to Artist", icon: "artist", action: () => app.go({ name: "artist", artist: artist.name }) },
          { label: "Change Picture…", icon: "edit", action: () => (app.artistEditing = artist.name) },
        ])}
      >
        <div class="avatar" style="--glow: {artist.albums[0]?.accent ?? 'transparent'}">
          {#if picture}
            <img src={artUrl(picture)} alt="" loading="lazy" />
          {:else}
            <Icon name="artist" size={40} />
          {/if}
        </div>
        <div class="name">{artist.name}</div>
        <div class="sub">{artist.albums.length} {artist.albums.length === 1 ? "album" : "albums"}, {artist.trackCount} songs</div>
      </button>
    {/each}
  </div>
</div>

<style>
  .count {
    margin: 6px 0 0;
    font-size: 13px;
  }
  .grid {
    margin-top: 28px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: 32px 24px;
  }
  .artist {
    text-align: center;
    min-width: 0;
    border-radius: var(--r-md);
  }
  .avatar {
    aspect-ratio: 1;
    border-radius: 50%;
    overflow: hidden;
    background: var(--surface);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow: 0 8px 22px rgba(0, 0, 0, 0.4);
    transition:
      transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1),
      box-shadow 0.25s;
  }
  .artist:hover .avatar {
    transform: scale(1.03);
    box-shadow:
      0 12px 28px rgba(0, 0, 0, 0.45),
      0 6px 30px color-mix(in srgb, var(--glow) 35%, transparent);
  }
  .avatar img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .name {
    margin-top: 12px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sub {
    font-size: 12.5px;
    color: var(--text-2);
  }
  /* Phones: two artists per row. */
  @media (max-width: 720px) {
    .grid {
      grid-template-columns: repeat(2, minmax(0, 1fr)) !important;
      gap: 20px 14px !important;
    }
  }
</style>

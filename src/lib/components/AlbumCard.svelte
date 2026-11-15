<script lang="ts">
  import { artUrl, type Album } from "#lib/api.js";
  import { app } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";

  let { album, size = 0 }: { album: Album; size?: number } = $props();
  const playing = $derived(app.currentAlbum?.id === album.id);
  const isPlaying = $derived(playing && app.status?.playing);
  const open = () => app.go({ name: "album", id: album.id });
</script>

<!-- Two buttons side by side, not one inside the other: "open" covers the whole card (its
     ::before), the play button sits above it. Enter and Space press whichever has the keyboard. -->
<div class="card" style={size ? `width: ${size}px` : ""} role="group" aria-label="{album.title} by {album.artist}" oncontextmenu={(e) => app.albumMenu(e, album)}>
  <div class="art-box">
    <div class="art" style="--glow: {album.accent ?? 'transparent'}">
      {#if album.artSmall}
        <img src={artUrl(album.artSmall)} alt="" loading="lazy" decoding="async" />
      {:else}
        <div class="placeholder"><Icon name="album" size={42} /></div>
      {/if}
    </div>
    <button
      class="play"
      class:visible={playing}
      aria-label="{isPlaying ? 'Pause' : 'Play'} {album.title}"
      onclick={() => {
        if (playing) app.togglePlay();
        else app.playAlbum(album);
      }}
    >
      <Icon name={isPlaying ? "pause" : "play"} size={17} />
    </button>
  </div>
  <button class="open" onclick={open} aria-label="Open {album.title} by {album.artist}">
    <span class="title">{album.title}</span>
    <span class="artist">{album.artist}</span>
  </button>
</div>

<style>
  .card {
    position: relative;
    isolation: isolate;
    min-width: 0;
    border-radius: var(--r-md);
  }
  .card:has(.open:focus-visible) .art {
    outline: 2px solid var(--accent);
    outline-offset: 3px;
  }
  .art-box {
    position: relative;
  }
  .art {
    position: relative;
    aspect-ratio: 1;
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--surface);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
    transition:
      transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1),
      box-shadow 0.25s;
  }
  .card:hover .art {
    transform: translateY(-3px);
    box-shadow:
      0 14px 30px rgba(0, 0, 0, 0.45),
      0 6px 30px color-mix(in srgb, var(--glow) 30%, transparent);
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .placeholder {
    height: 100%;
    display: grid;
    place-items: center;
    color: var(--text-3);
  }
  /* The whole card opens the album: the button's hit area is stretched over it. */
  .open {
    display: block;
    width: 100%;
    min-width: 0;
    text-align: left;
  }
  .open::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: 1;
    border-radius: var(--r-md);
  }
  .open:focus-visible {
    outline: none;
  }
  .play {
    position: absolute;
    z-index: 2;
    right: 10px;
    bottom: 10px;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--ink);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.45);
    opacity: 0;
    transform: translateY(6px);
    transition:
      opacity 0.18s,
      transform 0.18s;
  }
  .card:hover .play,
  .card:focus-within .play,
  .play.visible {
    opacity: 1;
    transform: none;
  }
  /* Lifts with the cover. */
  .card:hover .play {
    transform: translateY(-3px);
  }
  .play:hover {
    transform: translateY(-3px) scale(1.08) !important;
  }
  .title {
    display: block;
    margin-top: 10px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .artist {
    display: block;
    color: var(--text-2);
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>

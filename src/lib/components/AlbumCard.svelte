<script lang="ts">
  import { artUrl, type Album } from "$lib/api";
  import { app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";

  let { album, size = 0 }: { album: Album; size?: number } = $props();
  const playing = $derived(app.currentAlbum?.id === album.id);
  const isPlaying = $derived(playing && app.status?.playing);
  const open = () => app.go({ name: "album", id: album.id });
</script>

<div
  class="card"
  style={size ? `width: ${size}px` : ""}
  role="button"
  tabindex="0"
  aria-label="{album.title} by {album.artist}"
  onclick={open}
  onkeydown={(e) => e.key === "Enter" && open()}
  oncontextmenu={(e) => app.albumMenu(e, album)}
>
  <div class="art" style="--glow: {album.accent ?? 'transparent'}">
    {#if album.artSmall}
      <img src={artUrl(album.artSmall)} alt="" loading="lazy" decoding="async" />
    {:else}
      <div class="placeholder"><Icon name="album" size={42} /></div>
    {/if}
    <button
      class="play"
      class:visible={playing}
      aria-label="{isPlaying ? 'Pause' : 'Play'} {album.title}"
      onclick={(e) => {
        e.stopPropagation();
        if (playing) app.togglePlay();
        else app.playAlbum(album);
      }}
    >
      <Icon name={isPlaying ? "pause" : "play"} size={17} />
    </button>
  </div>
  <div class="title">{album.title}</div>
  <div class="artist">{album.artist}</div>
</div>

<style>
  .card {
    min-width: 0;
    outline: none;
    border-radius: var(--r-md);
  }
  .card:focus-visible .art {
    outline: 2px solid var(--accent);
    outline-offset: 3px;
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
  .play {
    position: absolute;
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
  .card:focus-visible .play,
  .play.visible {
    opacity: 1;
    transform: none;
  }
  .play:hover {
    transform: scale(1.08) !important;
  }
  .title {
    margin-top: 10px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .artist {
    color: var(--text-2);
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>

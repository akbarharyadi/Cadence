<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, artUrl, formatTime } from "$lib/api";
  import Icon from "$lib/components/Icon.svelte";
  import { app } from "$lib/store.svelte";

  onMount(() => {
    // The mini window is transparent so its rounded corners show.
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    app.init();
  });

  $effect(() => {
    const root = document.documentElement.style;
    root.setProperty("--accent", app.currentAlbum?.accent ?? "#8b7cff");
    root.setProperty("--tint", app.currentAlbum?.tint ?? "#1b1c33");
  });

  const t = $derived(app.current);
  const s = $derived(app.status);
  const pct = $derived(s?.duration ? (app.position / s.duration) * 100 : 0);
</script>

<div class="mini" data-tauri-drag-region>
  {#if app.currentAlbum?.artSmall}
    <div class="wash" style="background-image: url('{artUrl(app.currentAlbum.artSmall)}')" data-tauri-drag-region></div>
  {/if}
  <button class="art" onclick={() => api.showMain()} title="Open Cadence">
    {#if app.currentAlbum?.artSmall}<img src={artUrl(app.currentAlbum.artSmall)} alt="" />{:else}<Icon name="note" size={24} />{/if}
  </button>
  <div class="body" data-tauri-drag-region>
    <div class="text" data-tauri-drag-region>
      <div class="title" data-tauri-drag-region>{t?.title ?? "Nothing playing"}</div>
      <div class="artist" data-tauri-drag-region>{t?.artist ?? "Pick something in Cadence"}</div>
    </div>
    <div class="controls">
      <button class="btn" onclick={() => api.player("prev")} aria-label="Previous"><Icon name="prev" size={17} /></button>
      <button class="btn main" onclick={() => api.player("toggle")} aria-label={s?.playing ? "Pause" : "Play"}>
        <Icon name={s?.playing ? "pause" : "play"} size={18} />
      </button>
      <button class="btn" onclick={() => api.player("next")} aria-label="Next"><Icon name="next" size={17} /></button>
      <span class="time" data-tauri-drag-region>{formatTime(app.position)} / {formatTime(s?.duration ?? 0)}</span>
    </div>
  </div>
  <button class="close" onclick={() => getCurrentWindow().close()} aria-label="Close mini player" title="Close"><Icon name="close" size={13} /></button>
  <div class="progress"><div style="width: {pct}%"></div></div>
</div>

<style>
  .mini {
    position: relative;
    height: 100vh;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px 14px 12px;
    border-radius: 16px;
    overflow: hidden;
    background: color-mix(in srgb, color-mix(in srgb, var(--tint) 60%, var(--surface)) 94%, transparent);
    box-shadow: inset 0 0 0 1px var(--line-strong);
  }
  .wash {
    position: absolute;
    inset: -40%;
    background-size: cover;
    background-position: center;
    filter: blur(40px) saturate(1.4);
    opacity: 0.35;
  }
  .art {
    position: relative;
    width: 80px;
    height: 80px;
    flex: none;
    border-radius: 10px;
    overflow: hidden;
    background: var(--surface-2);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow: 0 6px 16px rgba(0, 0, 0, 0.45);
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .body {
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .title,
  .artist {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title {
    font-weight: 600;
    font-size: 14px;
  }
  .artist {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .btn {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: var(--text);
  }
  .btn:hover {
    background: var(--active);
  }
  .btn.main {
    width: 34px;
    height: 34px;
    background: var(--text);
    color: var(--ink);
  }
  .time {
    margin-left: auto;
    font-size: 11px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .close {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: var(--text-2);
    opacity: 0;
    transition: opacity 0.15s;
  }
  .mini:hover .close {
    opacity: 1;
  }
  .close:hover {
    background: var(--active);
    color: var(--text);
  }
  .progress {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: var(--line);
  }
  .progress div {
    height: 100%;
    background: var(--accent);
  }
</style>

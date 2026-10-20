<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, artUrl, formatTime, khz, qualityOf, type Lyrics } from "$lib/api";
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
  const duration = $derived(s?.duration ?? 0);
  const pct = $derived(duration ? (app.position / duration) * 100 : 0);
  const quality = $derived(
    s?.source ? `${qualityOf(s.source.codec, s.source.sampleRate)} ${s.source.bits ? `${s.source.bits}-bit/` : ""}${khz(s.source.sampleRate)}` : "",
  );

  // The line being sung, when the song has timed lyrics.
  let lyrics = $state<Lyrics | null>(null);
  const songId = $derived(t?.id);
  $effect(() => {
    const id = songId;
    lyrics = null;
    if (id) api.lyrics(id).then((l) => id === songId && (lyrics = l));
  });
  const line = $derived.by(() => {
    const lines = lyrics?.synced ?? [];
    let text = "";
    for (const l of lines) {
      if (l.time > app.position + 0.25) break;
      text = l.text;
    }
    return text;
  });

  function seek(e: MouseEvent) {
    const bar = e.currentTarget as HTMLElement;
    const r = bar.getBoundingClientRect();
    if (duration) app.seek(((e.clientX - r.left) / r.width) * duration);
  }
</script>

<div class="mini" data-tauri-drag-region>
  {#if app.currentAlbum?.artSmall}
    <div class="wash" style="background-image: url('{artUrl(app.currentAlbum.artSmall)}')" data-tauri-drag-region></div>
  {/if}
  <div class="veil" data-tauri-drag-region></div>

  <button class="art" onclick={() => api.showMain()} title="Open Cadence">
    {#if app.currentAlbum?.artSmall}<img src={artUrl(app.currentAlbum.artSmall)} alt="" />{:else}<Icon name="note" size={28} />{/if}
  </button>

  <div class="body" data-tauri-drag-region>
    <div class="title" data-tauri-drag-region>{t?.title ?? "Nothing playing"}</div>
    <div class="artist" data-tauri-drag-region>{t?.artist ?? "Pick something in Cadence"}</div>
    {#key line || quality}
      <div class="line" class:lyric={!!line} data-tauri-drag-region>
        {#if line}{line}{:else if quality}<Icon name="lossless" size={12} />{quality}{/if}
      </div>
    {/key}

    <div class="controls">
      <button class="btn" onclick={() => api.player("prev")} aria-label="Previous"><Icon name="prev" size={18} /></button>
      <button class="btn main" onclick={() => api.player("toggle")} aria-label={s?.playing ? "Pause" : "Play"}>
        <Icon name={s?.playing ? "pause" : "play"} size={18} />
      </button>
      <button class="btn" onclick={() => api.player("next")} aria-label="Next"><Icon name="next" size={18} /></button>
      <span class="eq" class:on={s?.playing} aria-hidden="true"><i></i><i></i><i></i><i></i></span>
      <span class="time" data-tauri-drag-region>{formatTime(app.position)} / {formatTime(duration)}</span>
    </div>
  </div>

  <div class="top-btns">
    <button onclick={() => api.showMain()} aria-label="Open Cadence" title="Open Cadence"><Icon name="expand" size={13} /></button>
    <button onclick={() => getCurrentWindow().close()} aria-label="Close mini player" title="Close"><Icon name="close" size={13} /></button>
  </div>

  <div class="progress" role="slider" tabindex="-1" aria-label="Seek" aria-valuemin={0} aria-valuemax={duration} aria-valuenow={app.position} onclick={seek} onkeydown={() => {}}>
    <div class="fill" style="width: {pct}%"></div>
  </div>
</div>

<style>
  .mini {
    position: relative;
    height: 100vh;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 16px 18px 14px;
    border-radius: 18px;
    overflow: hidden;
    background: color-mix(in srgb, var(--tint) 70%, #0d0e17);
    box-shadow: inset 0 0 0 1px var(--line-strong);
  }
  /* The cover, blurred, fills the player with the album's colours. */
  .wash {
    position: absolute;
    inset: -50%;
    background-size: cover;
    background-position: center;
    filter: blur(36px) saturate(1.6);
    opacity: 0.75;
    animation: drift 24s ease-in-out infinite alternate;
  }
  @keyframes drift {
    to {
      transform: scale(1.15) rotate(8deg);
    }
  }
  .veil {
    position: absolute;
    inset: 0;
    background: linear-gradient(100deg, rgba(10, 11, 20, 0.35), rgba(10, 11, 20, 0.6));
  }
  .art {
    position: relative;
    width: 104px;
    height: 104px;
    flex: none;
    border-radius: 12px;
    overflow: hidden;
    background: var(--surface-2);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow: 0 10px 24px rgba(0, 0, 0, 0.5);
    transition: transform 0.2s;
  }
  .art:hover {
    transform: scale(1.03);
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
  }
  .title,
  .artist,
  .line {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    padding-right: 44px;
  }
  .title {
    font-family: var(--display);
    font-weight: 700;
    font-size: 16px;
    letter-spacing: -0.01em;
  }
  .artist {
    font-size: 12.5px;
    color: rgba(255, 255, 255, 0.72);
  }
  .line {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-top: 5px;
    font-size: 12px;
    color: var(--gold);
    animation: line-in 0.35s ease-out;
  }
  .line.lyric {
    display: block;
    color: white;
    font-weight: 600;
    font-size: 12.5px;
  }
  @keyframes line-in {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 8px;
  }
  .btn {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: white;
    transition: background 0.15s, transform 0.1s;
  }
  .btn:hover {
    background: rgba(255, 255, 255, 0.14);
  }
  .btn:active {
    transform: scale(0.92);
  }
  .btn.main {
    width: 38px;
    height: 38px;
    background: white;
    color: #0d0e17;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
  }
  /* A small equaliser that dances while music plays. */
  .eq {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 14px;
    margin-left: 8px;
    opacity: 0;
    transition: opacity 0.3s;
  }
  .eq.on {
    opacity: 0.8;
  }
  .eq i {
    width: 3px;
    height: 4px;
    border-radius: 1px;
    background: var(--accent);
  }
  .eq.on i {
    animation: eq 0.8s ease-in-out infinite alternate;
  }
  .eq.on i:nth-child(2) {
    animation-delay: -0.25s;
  }
  .eq.on i:nth-child(3) {
    animation-delay: -0.5s;
  }
  .eq.on i:nth-child(4) {
    animation-delay: -0.15s;
  }
  @keyframes eq {
    from {
      height: 3px;
    }
    to {
      height: 14px;
    }
  }
  .time {
    margin-left: auto;
    font-size: 11px;
    color: rgba(255, 255, 255, 0.65);
    font-variant-numeric: tabular-nums;
  }
  .top-btns {
    position: absolute;
    top: 8px;
    right: 8px;
    display: flex;
    gap: 2px;
    opacity: 0;
    transition: opacity 0.15s;
  }
  .mini:hover .top-btns {
    opacity: 1;
  }
  .top-btns button {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: rgba(255, 255, 255, 0.75);
  }
  .top-btns button:hover {
    background: rgba(255, 255, 255, 0.14);
    color: white;
  }
  /* Click anywhere on the bar to jump; it grows on hover. */
  .progress {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 10px;
    display: flex;
    align-items: flex-end;
    cursor: pointer;
  }
  .progress::before {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: rgba(255, 255, 255, 0.12);
    transition: height 0.15s;
  }
  .fill {
    position: relative;
    height: 3px;
    background: var(--accent);
    box-shadow: 0 0 10px var(--accent);
    transition: height 0.15s;
  }
  .progress:hover::before,
  .progress:hover .fill {
    height: 5px;
  }
  @media (prefers-reduced-motion: reduce) {
    .wash,
    .eq.on i {
      animation: none;
    }
  }
</style>

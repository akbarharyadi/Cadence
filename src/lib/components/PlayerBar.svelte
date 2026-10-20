<script lang="ts">
  import { api, artUrl, formatTime, khz, qualityOf } from "$lib/api";
  import { app, signalPath } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import QualityBadge from "./QualityBadge.svelte";

  const s = $derived(app.status);
  const t = $derived(app.current);
  const album = $derived(app.currentAlbum);

  // Seek slider: follow playback unless the user is dragging it.
  let dragging = $state(false);
  let dragValue = $state(0);
  const shownPos = $derived(dragging ? dragValue : app.position);
  const duration = $derived(s?.duration ?? 0);

  // The slider is perceptual (squared); the top of the slider is exactly 1.0 = untouched samples.
  let volume = $state(1);
  let lastNonZero = 1;
  let volDragging = false;
  $effect(() => {
    if (s && !volDragging) volume = Math.sqrt(s.volume);
  });
  function setVolume(v: number) {
    volume = v;
    if (v > 0) lastNonZero = v;
    api.setVolume(v * v);
  }

  const repeatLabel = $derived(s?.repeat === "one" ? "Repeat one" : s?.repeat === "all" ? "Repeat all" : "Repeat off");
  function cycleRepeat() {
    api.setRepeat(s?.repeat === "off" ? "all" : s?.repeat === "all" ? "one" : "off");
  }

  const detail = $derived(s?.source ? `${s.source.bits ? `${s.source.bits}-bit/` : ""}${khz(s.source.sampleRate)}` : "");
  const quality = $derived(s?.source ? qualityOf(s.source.codec, s.source.sampleRate) : "");
  const path = $derived(signalPath(s));
</script>

<footer class="dock" class:idle={!t}>
  <div class="left">
    {#if t}
      <button class="art" onclick={() => (app.nowPlayingOpen = true)} aria-label="Open Now Playing" title="Open Now Playing">
        {#if album?.artSmall}<img src={artUrl(album.artSmall)} alt="" />{:else}<Icon name="note" size={22} />{/if}
        <span class="expand"><Icon name="expand" size={16} /></span>
      </button>
      <div class="meta">
        <button class="title" onclick={() => app.go({ name: "album", id: t.albumId })}>{t.title}</button>
        <button class="artist" onclick={() => app.go({ name: "artist", artist: app.artistOf(t.albumArtist) })}>{t.artist}</button>
        {#if quality}<QualityBadge {quality} {detail} />{/if}
      </div>
    {:else}
      <div class="art empty"><Icon name="note" size={22} /></div>
      <div class="meta"><span class="title idle-text">Pick something to play</span></div>
    {/if}
  </div>

  <div class="center">
    <div class="transport">
      <button class="icon-btn" class:on={s?.repeat !== "off"} onclick={cycleRepeat} aria-label={repeatLabel} title={repeatLabel}>
        <Icon name={s?.repeat === "one" ? "repeatOne" : "repeat"} size={17} />
      </button>
      <button class="icon-btn" onclick={() => api.player("prev")} aria-label="Previous" title="Previous (Ctrl+←)"><Icon name="prev" size={20} /></button>
      <button class="main-btn" onclick={() => api.player("toggle")} aria-label={s?.playing ? "Pause" : "Play"} title="{s?.playing ? 'Pause' : 'Play'} (Space)">
        <Icon name={s?.playing ? "pause" : "play"} size={22} />
      </button>
      <button class="icon-btn" onclick={() => api.player("next")} aria-label="Next" title="Next (Ctrl+→)"><Icon name="next" size={20} /></button>
      <button class="icon-btn" class:on={app.queueOpen} onclick={() => (app.queueOpen = !app.queueOpen)} aria-label="Up Next" title="Up Next">
        <Icon name="queue" size={18} />
      </button>
    </div>
    <div class="seek">
      <span class="time">{formatTime(shownPos)}</span>
      <input
        type="range"
        class="slider"
        min="0"
        max={Math.max(duration, 1)}
        step="0.1"
        value={shownPos}
        style="--pct: {duration ? (shownPos / duration) * 100 : 0}%"
        disabled={!t}
        onpointerdown={() => { dragging = true; dragValue = app.position; }}
        oninput={(e) => (dragValue = +e.currentTarget.value)}
        onchange={(e) => { app.seek(+e.currentTarget.value); dragging = false; }}
        aria-label="Seek"
      />
      <span class="time right">-{formatTime(Math.max(0, duration - shownPos))}</span>
    </div>
  </div>

  <div class="right">
    {#if path && s}
      <button
        class="path {path.tier}"
        class:clipping={s.output.clipping}
        onclick={() => app.go({ name: "sound" })}
        title={`${path.steps.join(", ")}, to ${s.output.device} at ${khz(s.output.sampleRate)}${s.output.clipping ? ". The output reached full scale." : ""}`}
      >
        <span class="led"></span>{s.output.clipping ? "Clipping" : path.label}
      </button>
    {/if}
    <button
      class="icon-btn"
      class:on={app.nowPlayingOpen && app.lyricsOpen}
      onclick={() => {
        app.lyricsOpen = true;
        app.nowPlayingOpen = true;
      }}
      aria-label="Lyrics"
      title="Lyrics"
      disabled={!t}
    >
      <Icon name="lyrics" size={18} />
    </button>
    <button class="icon-btn" onclick={() => api.toggleMini()} aria-label="Mini player" title="Mini player">
      <Icon name="mini" size={18} />
    </button>
    <button class="icon-btn" class:on={app.dsp?.eq.enabled} onclick={() => app.go({ name: "sound" })} aria-label="Sound" title="Sound: leveling, equalizer, crossfeed">
      <Icon name="eq" size={18} />
    </button>
    <button class="icon-btn" onclick={() => setVolume(volume > 0 ? 0 : lastNonZero)} aria-label={volume === 0 ? "Unmute" : "Mute"}>
      <Icon name={volume === 0 ? "mute" : volume < 0.5 ? "volumeLow" : "volume"} size={18} />
    </button>
    <input
      type="range"
      class="slider volume"
      min="0"
      max="1"
      step="0.01"
      value={volume}
      style="--pct: {volume * 100}%"
      onpointerdown={() => (volDragging = true)}
      onpointerup={() => (volDragging = false)}
      oninput={(e) => setVolume(+e.currentTarget.value)}
      aria-label="Volume"
      title="Volume {Math.round(volume * 100)}%"
    />
  </div>
</footer>

<style>
  /* A floating dock, tinted by the album that's playing. */
  .dock {
    position: fixed;
    left: 14px;
    right: 14px;
    bottom: 14px;
    z-index: 30;
    height: 80px;
    display: grid;
    grid-template-columns: minmax(220px, 1fr) minmax(340px, 1.3fr) minmax(220px, 1fr);
    align-items: center;
    gap: 20px;
    padding: 0 18px 0 12px;
    border-radius: 18px;
    background: color-mix(in srgb, color-mix(in srgb, var(--tint) 55%, var(--surface)) 82%, transparent);
    backdrop-filter: blur(36px) saturate(1.6);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 18px 50px rgba(0, 0, 0, 0.45);
  }
  .left {
    display: flex;
    align-items: center;
    gap: 13px;
    min-width: 0;
  }
  .art {
    position: relative;
    width: 56px;
    height: 56px;
    flex: none;
    border-radius: 9px;
    overflow: hidden;
    background: var(--surface-2);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.45);
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .expand {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.45);
    color: white;
    opacity: 0;
    transition: opacity 0.15s;
  }
  .art:hover .expand {
    opacity: 1;
  }
  .meta {
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
  }
  .title,
  .artist {
    max-width: 100%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }
  .title {
    font-weight: 600;
  }
  .artist {
    color: var(--text-2);
    font-size: 13px;
  }
  .title:hover,
  .artist:hover {
    text-decoration: underline;
  }
  .idle-text {
    color: var(--text-3);
    font-weight: 500;
  }
  .idle-text:hover {
    text-decoration: none;
  }
  .center {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .transport {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 12px;
  }
  .main-btn {
    width: 40px;
    height: 40px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--text);
    color: var(--ink);
    transition: transform 0.1s, background 0.2s;
  }
  .main-btn:hover {
    transform: scale(1.06);
  }
  .main-btn:active {
    transform: scale(0.94);
  }
  .seek {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .seek .slider {
    flex: 1;
  }
  .time {
    width: 42px;
    font-size: 11px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .time.right {
    text-align: right;
  }
  .right {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
  }
  .volume {
    width: 104px;
  }
  .path {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 26px;
    padding: 0 10px;
    margin-right: 6px;
    border-radius: 999px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-2);
    background: var(--hover);
    white-space: nowrap;
  }
  .path:hover {
    background: var(--active);
    color: var(--text);
  }
  .led {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  .path.lossless .led,
  .path.bitperfect .led {
    background: var(--gold);
    box-shadow: 0 0 8px var(--gold);
  }
  .path.clipping {
    color: var(--bad);
  }
  .path.clipping .led {
    background: var(--bad);
    box-shadow: 0 0 8px var(--bad);
  }
</style>

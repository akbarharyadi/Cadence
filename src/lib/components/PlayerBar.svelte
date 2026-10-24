<script lang="ts">
  import { fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { api, artUrl, formatTime, khz, qualityOf, type Album, type Track } from "$lib/api";
  import { app, signalPath } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import QualityBadge from "./QualityBadge.svelte";

  const s = $derived(app.status);
  const t = $derived(app.current);
  const album = $derived(app.currentAlbum);

  // Seek line: follows playback unless the user is dragging it.
  let dragging = $state(false);
  let dragValue = $state(0);
  const shownPos = $derived(dragging ? dragValue : app.position);
  const duration = $derived(s?.duration ?? 0);
  const pct = $derived(duration ? Math.min(100, (shownPos / duration) * 100) : 0);

  // Where the pointer is over the seek line, to show the time it would jump to.
  let hoverX = $state<number | null>(null);
  let seekEl = $state<HTMLDivElement>();
  const hoverTime = $derived(hoverX != null && duration ? hoverX * duration : 0);

  function fraction(e: PointerEvent): number {
    const r = seekEl!.getBoundingClientRect();
    return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
  }
  function seekDown(e: PointerEvent) {
    if (!duration) return;
    seekEl!.setPointerCapture(e.pointerId);
    dragging = true;
    dragValue = fraction(e) * duration;
  }
  function seekMove(e: PointerEvent) {
    hoverX = fraction(e);
    if (dragging) dragValue = hoverX * duration;
  }
  function seekUp(e: PointerEvent) {
    if (!dragging) return;
    app.seek(fraction(e) * duration);
    dragging = false;
  }
  function seekKey(e: KeyboardEvent) {
    const step = e.key === "ArrowRight" ? 5 : e.key === "ArrowLeft" ? -5 : 0;
    if (!step) return;
    e.preventDefault();
    e.stopPropagation();
    app.seek(Math.min(duration, Math.max(0, app.position + step)));
  }

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

  // The songs either side of this one, listed over the Previous / Next buttons.
  type Entry = { track: Track; album?: Album; index: number };
  function around(step: 1 | -1, count: number): Entry[] {
    if (!s || s.index == null || (step === 1 && s.repeat === "one")) return [];
    const out: Entry[] = [];
    const n = s.queue.length;
    for (let k = 1; k <= count && k < n; k++) {
      let i = s.index + step * k;
      if (s.repeat === "all") i = (i + n) % n;
      if (i < 0 || i >= n) break;
      const track = app.trackById.get(s.queue[i]);
      if (track) out.push({ track, album: app.albumById.get(track.albumId), index: i });
    }
    return out;
  }
  let peek = $state<"prev" | "next" | null>(null);

  /** The list grows out of the button: a small rise, scale and fade on top of its own offset. */
  function pop(node: Element, { duration = 240 }: { duration?: number } = {}) {
    const base = getComputedStyle(node).transform;
    const offset = base === "none" ? "" : base;
    return {
      duration,
      easing: cubicOut,
      css: (k: number) => `opacity: ${k}; transform: ${offset} translateY(${(1 - k) * 10}px) scale(${0.96 + 0.04 * k});`,
    };
  }
  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  function openPeek(which: "prev" | "next") {
    clearTimeout(closeTimer);
    peek = which;
  }
  // A short delay, so the pointer can move from the button onto the list.
  function closePeek() {
    clearTimeout(closeTimer);
    closeTimer = setTimeout(() => (peek = null), 180);
  }
  // Near the end of a song, the next one shows by itself.
  const ending = $derived(!!s?.playing && duration > 30 && duration - app.position < 15);
  const shown = $derived(peek ?? (ending && around(1, 1).length ? "next" : null));
  const list = $derived(shown ? around(shown === "next" ? 1 : -1, peek ? 5 : 1) : []);
</script>

<!-- The player appears once something is queued, and slides away when the queue is cleared. -->
{#if t}
  <footer class="dock" transition:fly={{ y: 40, duration: 320, easing: cubicOut }}>
    <div class="clip" aria-hidden="true">
      {#if album?.artSmall}<div class="glow" style="background-image: url('{artUrl(album.artSmall)}')"></div>{/if}
    </div>

    <!-- Seek: the whole top edge of the player. -->
    <div
      class="seek"
      class:active={dragging}
      bind:this={seekEl}
      role="slider"
      tabindex="0"
      aria-label="Seek"
      aria-valuemin={0}
      aria-valuemax={Math.round(duration)}
      aria-valuenow={Math.round(shownPos)}
      aria-valuetext="{formatTime(shownPos)} of {formatTime(duration)}"
      onpointerdown={seekDown}
      onpointermove={seekMove}
      onpointerup={seekUp}
      onpointerleave={() => (hoverX = null)}
      onkeydown={seekKey}
    >
      <div class="rail"><div class="fill" style="transform: scaleX({pct / 100})"></div></div>
      <div class="thumb-track" style="transform: translateX({pct}%)"><div class="thumb"></div></div>
      {#if hoverX != null && duration}
        <div class="tip" style="left: {hoverX * 100}%">{formatTime(dragging ? dragValue : hoverTime)}</div>
      {/if}
    </div>

    <div class="left">
      <button class="art" onclick={() => (app.nowPlayingOpen = true)} aria-label="Open Now Playing" title="Open Now Playing">
        {#if album?.artSmall}<img src={artUrl(album.artSmall)} alt="" />{:else}<Icon name="note" size={22} />{/if}
        <span class="expand"><Icon name="expand" size={16} /></span>
      </button>
      <div class="meta" role="group" oncontextmenu={(e) => app.trackMenu(e, [t])}>
        {#key t.id}
          <div class="meta-in">
            <button class="title" onclick={() => app.go({ name: "album", id: t.albumId })}>{t.title}</button>
            <button class="artist" onclick={() => app.go({ name: "artist", artist: app.artistOf(t.albumArtist) })}>{t.artist}</button>
            {#if quality}<span class="badge"><QualityBadge {quality} {detail} /></span>{/if}
          </div>
        {/key}
      </div>
      {#if app.needsInfo(t)}
        <button class="icon-btn fix" onclick={() => (app.fixing = t)} aria-label="Fix song info" title="Fix song info: find the title, artist, album and cover">
          <Icon name="sparkle" size={16} />
        </button>
      {/if}
      <button class="icon-btn skip" onclick={() => app.setSkipped([t], true)} aria-label="Skip this song from now on" title="Skip this song from now on: it's left out when albums, playlists and shuffles play">
        <Icon name="skip" size={16} />
      </button>
    </div>

    <div class="center">
      <span class="time">{formatTime(shownPos)}</span>
      <div class="transport">
        <button class="icon-btn" class:on={s?.repeat !== "off"} onclick={cycleRepeat} aria-label={repeatLabel} title={repeatLabel}>
          <Icon name={s?.repeat === "one" ? "repeatOne" : "repeat"} size={17} />
        </button>
        <button class="icon-btn" onclick={() => api.player("prev")} onpointerenter={() => openPeek("prev")} onpointerleave={closePeek} aria-label="Previous"><Icon name="prev" size={20} /></button>
        <button class="main-btn" class:playing={s?.playing} onclick={() => api.player("toggle")} aria-label={s?.playing ? "Pause" : "Play"} title="{s?.playing ? 'Pause' : 'Play'} (Space)">
          <Icon name={s?.playing ? "pause" : "play"} size={22} />
        </button>
        <button class="icon-btn" onclick={() => api.player("next")} onpointerenter={() => openPeek("next")} onpointerleave={closePeek} aria-label="Next"><Icon name="next" size={20} /></button>
        <button class="icon-btn" class:on={app.queueOpen} onclick={() => (app.queueOpen = !app.queueOpen)} aria-label="Up Next" title="Up Next">
          <Icon name="queue" size={18} />
        </button>
      </div>
      {#if list.length}
        {#key shown}
        <div
          class="peek {shown}"
          role="dialog"
          aria-label={shown === "next" ? "Up next" : "Played before"}
          tabindex="-1"
          onpointerenter={() => peek && openPeek(peek)}
          onpointerleave={closePeek}
          in:pop
          out:pop={{ duration: 160 }}
        >
          <div class="peek-head">
            <span>{shown === "next" ? (ending && !peek ? `Up next in ${Math.ceil(duration - app.position)}s` : "Up next") : "Played before"}</span>
            {#if peek}
              <button class="link" onclick={() => { app.queueOpen = true; peek = null; }}>Show all</button>
            {/if}
          </div>
          {#each list as e, i (e.index)}
            <button class="peek-row" class:first={i === 0} in:fly={{ y: 6, duration: 260, delay: 40 + i * 28, easing: cubicOut }} onclick={() => { api.jump(e.index); peek = null; }} title="Play {e.track.title}">
              <span class="peek-art">{#if e.album?.artSmall}<img src={artUrl(e.album.artSmall)} alt="" />{:else}<Icon name="note" size={14} />{/if}</span>
              <span class="peek-text">
                <span class="peek-title">{e.track.title}</span>
                <span class="peek-artist">{e.track.artist}</span>
              </span>
              <span class="peek-time">{formatTime(e.track.duration)}</span>
            </button>
          {/each}
        </div>
        {/key}
      {/if}
      <span class="time right">-{formatTime(Math.max(0, duration - shownPos))}</span>
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
      <div class="tools">
        <button
          class="icon-btn"
          class:on={app.nowPlayingOpen && app.lyricsOpen}
          onclick={() => {
            app.lyricsOpen = true;
            app.nowPlayingOpen = true;
          }}
          aria-label="Lyrics"
          title="Lyrics"
        >
          <Icon name="lyrics" size={18} />
        </button>
        <button class="icon-btn" onclick={() => api.toggleMini()} aria-label="Mini player" title="Mini player">
          <Icon name="mini" size={18} />
        </button>
        <button class="icon-btn" class:on={app.dsp?.eq.enabled} onclick={() => app.go({ name: "sound" })} aria-label="Sound" title="Sound: leveling, equalizer, crossfeed">
          <Icon name="eq" size={18} />
        </button>
      </div>
      <div class="vol">
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
    </div>
  </footer>
{/if}

<style>
  /* A floating player, tinted by the album that's playing. */
  .dock {
    position: fixed;
    left: 14px;
    right: 14px;
    bottom: 14px;
    z-index: 30;
    height: 78px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
    align-items: center;
    gap: 24px;
    padding: 0 18px 0 11px;
    border-radius: 18px;
    background: color-mix(in srgb, color-mix(in srgb, var(--tint) 55%, var(--surface)) 84%, transparent);
    backdrop-filter: blur(36px) saturate(1.6);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 18px 50px rgba(0, 0, 0, 0.45);
  }
  .clip {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    overflow: hidden;
    pointer-events: none;
  }
  /* The cover's colours, softly, from the left. */
  .glow {
    position: absolute;
    left: -60px;
    top: -60px;
    width: 420px;
    height: 200px;
    background-size: cover;
    background-position: center;
    filter: blur(50px) saturate(1.7);
    opacity: 0.35;
    pointer-events: none;
    mask-image: linear-gradient(to right, black, transparent);
  }

  /* ---------- seek line ---------- */
  .seek {
    position: absolute;
    left: 18px;
    right: 18px;
    top: 0;
    height: 14px;
    z-index: 2;
    cursor: pointer;
    touch-action: none;
  }
  .seek:focus-visible {
    outline: none;
  }
  .rail {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    height: 3px;
    border-radius: 0 0 3px 3px;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.1);
    transition: height 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .fill {
    height: 100%;
    transform-origin: left;
    will-change: transform;
    background: var(--accent);
    box-shadow: 0 0 12px color-mix(in srgb, var(--accent) 70%, transparent);
  }
  .seek:hover .rail,
  .seek.active .rail,
  .seek:focus-visible .rail {
    height: 5px;
  }
  /* Moves with transforms only, so it glides without relayout. */
  .thumb-track {
    position: absolute;
    inset: 0;
    pointer-events: none;
    will-change: transform;
  }
  .thumb {
    position: absolute;
    left: 0;
    top: 0;
    width: 12px;
    height: 12px;
    margin: -3.5px 0 0 -6px;
    border-radius: 50%;
    background: white;
    box-shadow: 0 1px 6px rgba(0, 0, 0, 0.5);
    transform: scale(0);
    transition: transform 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .seek:hover .thumb,
  .seek.active .thumb,
  .seek:focus-visible .thumb {
    transform: scale(1);
  }
  .tip {
    position: absolute;
    top: 12px;
    transform: translateX(-50%);
    padding: 2px 7px;
    border-radius: 6px;
    background: rgba(10, 11, 20, 0.85);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: white;
    pointer-events: none;
    white-space: nowrap;
  }

  /* ---------- song ---------- */
  .left {
    position: relative;
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
    border-radius: 10px;
    overflow: hidden;
    background: var(--surface-2);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow: 0 6px 16px rgba(0, 0, 0, 0.45);
    transition: transform 0.15s;
  }
  .art:hover {
    transform: scale(1.04);
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
  /* Only with a mouse: on touch screens the hover state sticks after a tap. */
  @media (hover: hover) {
    .art:hover .expand {
      opacity: 1;
    }
  }
  .meta {
    min-width: 0;
    display: grid;
  }
  .meta-in {
    grid-area: 1 / 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    animation: meta-in 0.35s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  @keyframes meta-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
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
    font-weight: 650;
    font-size: 14.5px;
  }
  .artist {
    color: var(--text-2);
    font-size: 13px;
  }
  .title:hover,
  .artist:hover {
    text-decoration: underline;
  }
  .badge {
    margin-top: 2px;
  }
  /* Untagged songs: always shown, so the fix is easy to find. */
  .fix {
    flex: none;
    width: 30px;
    height: 30px;
    margin-left: -4px;
    color: var(--accent);
  }
  /* Shown when the pointer is over the song (or it has keyboard focus). */
  .skip {
    flex: none;
    width: 30px;
    height: 30px;
    margin-left: -4px;
    color: var(--text-2);
    opacity: 0;
    transition: opacity 0.15s;
  }
  .left:hover .skip,
  .skip:focus-visible {
    opacity: 1;
  }
  @media (hover: none) {
    .skip {
      display: none;
    }
  }

  /* ---------- transport ---------- */
  .center {
    position: relative;
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .transport {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .main-btn {
    width: 44px;
    height: 44px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--text);
    color: var(--ink);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
    transition: transform 0.1s, box-shadow 0.3s;
  }
  .main-btn.playing {
    box-shadow:
      0 4px 16px rgba(0, 0, 0, 0.35),
      0 0 0 5px color-mix(in srgb, var(--accent) 22%, transparent);
  }
  .main-btn:hover {
    transform: scale(1.06);
  }
  .main-btn:active {
    transform: scale(0.94);
  }
  .time {
    width: 40px;
    font-size: 11.5px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
  .time.right {
    text-align: left;
  }

  /* The songs before / after, above the buttons. */
  .peek {
    position: absolute;
    bottom: calc(100% + 22px);
    z-index: 5;
    width: 300px;
    padding: 10px 6px 6px;
    border-radius: 16px;
    /* Solid: a blur can't see through the player's own blur. */
    background: color-mix(in srgb, var(--tint) 30%, var(--panel));
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 16px 40px rgba(0, 0, 0, 0.55);
  }
  /* A bridge over the gap, so the list stays open on the way up from the button. */
  .peek::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    top: 100%;
    height: 24px;
  }
  .peek.prev {
    left: 50%;
    transform: translateX(-75%);
    transform-origin: 75% 100%;
  }
  .peek.next {
    left: 50%;
    transform: translateX(-25%);
    transform-origin: 25% 100%;
  }
  .peek-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 0 8px 6px;
    font-size: 11.5px;
    font-weight: 650;
    color: var(--accent);
  }
  .link {
    font-size: 11.5px;
    color: var(--text-2);
  }
  .link:hover {
    color: var(--text);
    text-decoration: underline;
  }
  .peek-row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 8px;
    border-radius: 10px;
    text-align: left;
  }
  .peek-row:hover {
    background: var(--hover);
  }
  .peek-art {
    width: 34px;
    height: 34px;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    display: grid;
    place-items: center;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .first .peek-art {
    width: 46px;
    height: 46px;
    border-radius: 8px;
  }
  .peek-art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .peek-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .peek-title,
  .peek-artist {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .peek-title {
    font-weight: 600;
    font-size: 13px;
  }
  .first .peek-title {
    font-size: 14px;
  }
  .peek-artist {
    font-size: 12px;
    color: var(--text-2);
  }
  .peek-time {
    font-size: 11px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }

  /* ---------- tools ---------- */
  .right {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 10px;
    min-width: 0;
  }
  .tools,
  .vol {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .volume {
    width: 96px;
  }
  .path {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 26px;
    padding: 0 10px;
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

  /* Narrower windows: drop the signal chip, then the times. */
  @media (max-width: 1180px) {
    .path {
      display: none;
    }
  }
  @media (max-width: 960px) {
    .time {
      display: none;
    }
    .volume {
      width: 70px;
    }
  }

  /* Phones: song, play and next above the tab bar; the line on top still shows (and seeks). */
  @media (max-width: 720px) {
    .dock {
      left: 8px;
      right: 8px;
      bottom: calc(64px + env(safe-area-inset-bottom));
      height: 64px;
      grid-template-columns: minmax(0, 1fr) auto;
      gap: 8px;
      padding: 0 8px;
      border-radius: 16px;
    }
    .art {
      width: 46px;
      height: 46px;
    }
    .badge,
    .peek,
    .right,
    .time,
    .transport > :not(.main-btn):not([aria-label="Next"]) {
      display: none;
    }
    .main-btn {
      width: 40px;
      height: 40px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .meta-in {
      animation: none;
    }
    .thumb,
    .rail {
      transition: none;
    }
  }
</style>

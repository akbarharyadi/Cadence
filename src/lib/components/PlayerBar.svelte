<script lang="ts">
  import { fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { api, artUrl, formatTime, khz, qualityOf, type Album, type Track } from "#lib/api.js";
  import { app, shuffled, signalPath, type MenuItem } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";
  import QualityBadge from "./QualityBadge.svelte";
  import SleepLabel from "./SleepLabel.svelte";

  /** "top": the desktop window's top bar. "bottom": a bar along the bottom of the desktop window
   * (Settings > Appearance). "dock": the phone's player above the tab bar. */
  let { placement = "dock" }: { placement?: "top" | "bottom" | "dock" } = $props();

  const s = $derived(app.status);
  const t = $derived(app.current);
  const album = $derived(app.currentAlbum);

  // Seek line: follows playback unless the user is dragging it.
  let dragging = $state(false);
  let dragValue = $state(0);
  const shownPos = $derived(dragging ? dragValue : app.position);
  const duration = $derived(s?.duration ?? 0);
  const pct = $derived(duration ? Math.min(100, (shownPos / duration) * 100) : 0);
  // The line glides between the player's four updates a second only when an update moves it at
  // least two screen pixels (short songs, wide windows). Moving less, it already looks continuous,
  // and gliding would redraw the page on every frame: on a phone, 47 frames a second and most of
  // the app's processor time, just for this line.
  let seekWidth = $state(0);
  const glide = $derived(!!s?.playing && !dragging && duration > 0 && (seekWidth * devicePixelRatio) / 4 / duration >= 2);

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

  const repeatLabel = $derived(
    s?.repeat === "one" ? "Repeat: this song (Ctrl+R)" : s?.repeat === "all" ? "Repeat: all (Ctrl+R)" : "Repeat: off (Ctrl+R)",
  );
  const shuffleLabel = $derived(s?.shuffle ? "Shuffle: on (Ctrl+S)" : "Shuffle: off (Ctrl+S)");

  /** A menu dropping from (or, at the bottom, rising above) the button that opened it. */
  function menuFrom(e: MouseEvent, items: MenuItem[]) {
    menuAt((e.currentTarget as HTMLElement).getBoundingClientRect(), items);
  }
  function menuAt(r: DOMRect, items: MenuItem[]) {
    app.menu = { x: r.left, y: placement === "top" ? r.bottom + 6 : Math.max(8, r.top - 6 - items.length * 36), items };
  }
  /** The song's own menu (play next, add to playlist, fix info, skip from now on…). */
  function songMenu(e: MouseEvent) {
    if (!t) return;
    const at = (e.currentTarget as HTMLElement).getBoundingClientRect();
    app.trackMenu(e, [t]);
    if (app.menu && e.type === "click") app.menu = { ...app.menu, x: at.left, y: placement === "top" ? at.bottom + 6 : Math.max(8, at.top - 6 - app.menu.items.length * 36) };
  }
  function toolsMenu(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    menuFrom(e, [
      { label: "Mini Player", icon: "mini", action: () => api.toggleMini() },
      { label: app.sleepActive ? "Sleep Timer (on)…" : "Sleep Timer…", icon: "moon", action: () => menuAt(r, app.sleepItems()) },
      { label: "Sound Settings", icon: "eq", action: () => app.go({ name: "sound" }) },
      app.playerPosition === "top"
        ? { label: "Move Player to the Bottom", icon: "down", action: () => app.setPlayerPosition("bottom") }
        : { label: "Move Player to the Top", icon: "display", action: () => app.setPlayerPosition("top") },
    ]);
  }
  function shuffleLibrary() {
    if (app.tracks.length) app.playTracks(shuffled(app.tracks));
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

  /** The list grows out of the button: a small rise (or drop, under the top bar), scale and fade on top of its own offset. */
  function pop(node: Element, { duration = 240 }: { duration?: number } = {}) {
    const base = getComputedStyle(node).transform;
    const offset = base === "none" ? "" : base;
    const shift = placement === "top" ? -8 : 10;
    return {
      duration,
      easing: cubicOut,
      css: (k: number) => `opacity: ${k}; transform: ${offset} translateY(${(1 - k) * shift}px) scale(${0.96 + 0.04 * k});`,
    };
  }
  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  let openTimer: ReturnType<typeof setTimeout> | undefined;
  // Only when the pointer rests on the button, so passing over it doesn't pop a list open.
  function openPeek(which: "prev" | "next") {
    clearTimeout(closeTimer);
    clearTimeout(openTimer);
    if (peek) peek = which;
    else openTimer = setTimeout(() => (peek = which), 450);
  }
  // A short delay, so the pointer can move from the button onto the list.
  function closePeek() {
    clearTimeout(openTimer);
    clearTimeout(closeTimer);
    closeTimer = setTimeout(() => (peek = null), 180);
  }
  // Near the end of a song, the next one shows by itself.
  const ending = $derived(!!s?.playing && duration > 30 && duration - app.position < 15);
  const shown = $derived(peek ?? (ending && around(1, 1).length ? "next" : null));
  const list = $derived(shown ? around(shown === "next" ? 1 : -1, peek ? 5 : 1) : []);
</script>

{#snippet peekList(top: boolean)}
  {#if list.length}
    {#key shown}
    <div
      class="peek {shown}"
      class:t={top}
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
        <button class="peek-row" class:first={i === 0} in:fly={{ y: top ? -6 : 6, duration: 260, delay: 40 + i * 28, easing: cubicOut }} onclick={() => { api.jump(e.index); peek = null; }} title="Play {e.track.title}">
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
{/snippet}

{#snippet seekLine()}
  <div
    class="seek"
    class:active={dragging}
    class:glide
    bind:this={seekEl}
    bind:clientWidth={seekWidth}
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
{/snippet}

{#snippet seekRow()}
  <div class="seek-row">
    <span class="time">{formatTime(shownPos)}</span>
    {@render seekLine()}
    <span class="time r">-{formatTime(Math.max(0, duration - shownPos))}</span>
  </div>
{/snippet}

<!-- Shuffle and repeat flank the three buttons, as in most players; when on, they light up in the
     accent colour with a dot underneath. -->
{#snippet transport(big: boolean)}
  <div class="transport">
    <button class="icon-btn toggle" class:on={s?.shuffle} onclick={() => app.toggleShuffle()} disabled={!t} aria-label={shuffleLabel} aria-pressed={!!s?.shuffle} title={shuffleLabel}>
      <Icon name="shuffle" size={big ? 18 : 16} />
    </button>
    <button class="icon-btn" onclick={() => api.player("prev")} onpointerenter={() => openPeek("prev")} onpointerleave={closePeek} disabled={!t} aria-label="Previous" title="Previous (Ctrl+←)"><Icon name="prev" size={big ? 20 : 19} /></button>
    <button class="main-btn" class:playing={s?.playing} onclick={() => api.player("toggle")} disabled={!t} aria-label={s?.playing ? "Pause" : "Play"} title="{s?.playing ? 'Pause' : 'Play'} (Space)">
      <Icon name={s?.playing ? "pause" : "play"} size={big ? 22 : 18} />
    </button>
    <button class="icon-btn next" onclick={() => api.player("next")} onpointerenter={() => openPeek("next")} onpointerleave={closePeek} disabled={!t} aria-label="Next" title="Next (Ctrl+→)"><Icon name="next" size={big ? 20 : 19} /></button>
    <button class="icon-btn toggle" class:on={s?.repeat !== "off"} onclick={() => app.cycleRepeat()} disabled={!t} aria-label={repeatLabel} aria-pressed={s?.repeat !== "off"} title={repeatLabel}>
      <Icon name={s?.repeat === "one" ? "repeatOne" : "repeat"} size={big ? 18 : 16} />
    </button>
  </div>
{/snippet}

<!-- The song's own buttons: add to a playlist, and its menu (where "Skip this song from now on"
     lives, away from Next). Untagged songs also get the fix button, in the accent colour. -->
{#snippet songActions()}
  {#if t}
    {#if app.needsInfo(t)}
      <button class="icon-btn fix" onclick={() => (app.fixing = t)} aria-label="Fix song info" title="Fix song info: find the title, artist, album and cover">
        <Icon name="sparkle" size={16} />
      </button>
    {/if}
    <button
      class="icon-btn act heart"
      class:loved={app.isLoved(t)}
      onclick={() => app.toggleLoved(t)}
      aria-label={app.isLoved(t) ? "Remove from Favorites" : "Add to Favorites"}
      aria-pressed={app.isLoved(t)}
      title={app.isLoved(t) ? "Remove from Favorites" : "Add to Favorites"}
    >
      <Icon name={app.isLoved(t) ? "heartFill" : "heart"} size={17} />
    </button>
    <button class="icon-btn act" onclick={() => (app.picker = t ? [t] : null)} aria-label="Add to a playlist" title="Add to a playlist">
      <Icon name="plus" size={17} />
    </button>
    <button class="icon-btn act" onclick={songMenu} aria-label="More for this song" title="More for this song: play next, go to album, skip from now on…">
      <Icon name="more" size={17} />
    </button>
  {/if}
{/snippet}

{#snippet rightTools()}
  {#if path && s}
    <button
      class="path {path.tier}"
      class:clipping={s.output.clipping}
      onclick={() => app.go({ name: "sound" })}
      title={`Sound: ${path.steps.join(", ")}, to ${s.output.device} at ${khz(s.output.sampleRate)}${s.output.clipping ? ". The output reached full scale." : ""}. Click for sound settings.`}
    >
      <span class="led"></span>{s.output.clipping ? "Clipping" : path.label}
    </button>
  {/if}
  <div class="tools">
    {#if app.sleepActive}
      <button class="icon-btn on sleep" onclick={(e) => menuFrom(e, app.sleepItems())} aria-label="Sleep timer" title="Sleep timer: click to change or turn off">
        <Icon name="moon" size={16} /><span><SleepLabel /></span>
      </button>
    {/if}
    <button
      class="icon-btn"
      class:on={app.nowPlayingOpen && app.lyricsOpen}
      disabled={!t}
      onclick={() => {
        app.lyricsOpen = true;
        app.nowPlayingOpen = true;
      }}
      aria-label="Lyrics"
      title="Lyrics (Ctrl+L)"
    >
      <Icon name="lyrics" size={18} />
    </button>
    <button class="icon-btn" class:on={app.queueOpen} onclick={() => (app.queueOpen = !app.queueOpen)} aria-label="Up Next" title="Up Next (Ctrl+U)">
      <Icon name="queue" size={18} />
    </button>
    <button class="icon-btn" onclick={toolsMenu} aria-label="More" title="More: mini player, sound settings, player position">
      <Icon name="more" size={18} />
    </button>
  </div>
  <div class="vol">
    <button class="icon-btn" onclick={() => setVolume(volume > 0 ? 0 : lastNonZero)} aria-label={volume === 0 ? "Unmute" : "Mute"} title={volume === 0 ? "Unmute" : "Mute"}>
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
{/snippet}

<!-- Nothing playing: say so, and offer something to play. -->
{#snippet idle()}
  {#if app.tracks.length}
    <button class="idle-btn" onclick={shuffleLibrary} title="Play your whole library in a random order">
      <span class="idle-text">Nothing playing</span>
      <span class="idle-cta"><Icon name="shuffle" size={14} />Shuffle your library</span>
    </button>
  {:else}
    <div class="idle-btn quiet" data-tauri-drag-region><span class="idle-text">Nothing playing</span></div>
  {/if}
{/snippet}

{#if placement === "top"}
  <!-- Desktop: the player is the window's top bar: the buttons on the left, the song with its
       times and seek line in the middle, the tools on the right (the window's own buttons follow
       them, see TitleBar). Nothing floats over the page. -->
  <header class="top" data-tauri-drag-region>
    <div class="t-left" data-tauri-drag-region>
      {@render transport(false)}
      {@render peekList(true)}
    </div>

    <div class="lcd" class:idle={!t}>
      {#if t}
        <!-- The whole display opens Now Playing; the title, artist and album are links on top. -->
        <button class="lcd-open" onclick={() => (app.nowPlayingOpen = true)} aria-label="Open Now Playing" title="Open Now Playing"></button>
        <span class="art" aria-hidden="true">
          {#if album?.artSmall}<img src={artUrl(album.artSmall)} alt="" />{:else}<Icon name="note" size={18} />{/if}
          <span class="expand"><Icon name="expand" size={14} /></span>
        </span>
        <div class="lcd-body">
          {#if album?.artSmall}<div class="lcd-glow" style="background-image: url('{artUrl(album.artSmall)}')" aria-hidden="true"></div>{/if}
          <div class="lcd-row">
            <div class="meta" role="group" oncontextmenu={(e) => t && app.trackMenu(e, [t])}>
              {#key t.id}
                <div class="meta-in">
                  <button class="title" onclick={() => t && app.go({ name: "album", id: t.albumId })} title="Go to the album">{t.title}</button>
                  <span class="sub">
                    <button class="artist" onclick={() => t && app.go({ name: "artist", artist: app.artistOf(t.albumArtist) })} title="Go to the artist">{t.artist}</button>{#if t.album}<span class="dash">—</span><button class="album-link" onclick={() => t && app.go({ name: "album", id: t.albumId })} title="Go to the album">{t.album}</button>{/if}
                  </span>
                </div>
              {/key}
            </div>
            <div class="lcd-side">
              {#if quality}<QualityBadge {quality} {detail} />{/if}
              {@render songActions()}
            </div>
          </div>
          {@render seekRow()}
        </div>
      {:else}
        {@render idle()}
      {/if}
    </div>

    <div class="t-right" data-tauri-drag-region>
      {@render rightTools()}
    </div>
  </header>
{:else if placement === "bottom"}
  <!-- Desktop, Settings > Appearance > Player at the bottom: the song on the left, the buttons with
       the seek line under them in the middle, the tools on the right. -->
  <footer class="bar">
    <div class="b-left">
      {#if t}
        <button class="art" onclick={() => (app.nowPlayingOpen = true)} aria-label="Open Now Playing" title="Open Now Playing">
          {#if album?.artSmall}<img src={artUrl(album.artSmall)} alt="" />{:else}<Icon name="note" size={22} />{/if}
          <span class="expand"><Icon name="expand" size={16} /></span>
        </button>
        <div class="meta" role="group" oncontextmenu={(e) => t && app.trackMenu(e, [t])}>
          {#key t.id}
            <div class="meta-in">
              <button class="title" onclick={() => t && app.go({ name: "album", id: t.albumId })} title="Go to the album">{t.title}</button>
              <button class="artist" onclick={() => t && app.go({ name: "artist", artist: app.artistOf(t.albumArtist) })} title="Go to the artist">{t.artist}</button>
            </div>
          {/key}
        </div>
        <div class="b-actions">{@render songActions()}</div>
      {:else}
        {@render idle()}
      {/if}
    </div>
    <div class="b-center">
      <div class="b-transport">
        {@render transport(true)}
        {@render peekList(false)}
      </div>
      {@render seekRow()}
    </div>
    <div class="b-right">
      {#if quality}<QualityBadge {quality} {detail} />{/if}
      {@render rightTools()}
    </div>
  </footer>
{:else if t}
  <!-- Phones: the player sits above the tab bar once something is queued, and slides away when
       the queue is cleared. -->
  <footer class="dock" transition:fly={{ y: 40, duration: 320, easing: cubicOut }}>
    <div class="clip" aria-hidden="true">
      {#if album?.artSmall}<div class="glow" style="background-image: url('{artUrl(album.artSmall)}')"></div>{/if}
    </div>

    <!-- Seek: the whole top edge of the player. -->
    {@render seekLine()}

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
    </div>

    <div class="center">
      <span class="time">{formatTime(shownPos)}</span>
      {@render transport(true)}
      {@render peekList(false)}
      <span class="time right">-{formatTime(Math.max(0, duration - shownPos))}</span>
    </div>

    <div class="right">
      {@render rightTools()}
    </div>
  </footer>
{/if}


<style>
  /* Phones: the player above the tab bar, tinted by the album that's playing (sized in the
     phone rules below). */
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
  /* The position comes four times a second; in between, the line glides when it would visibly
     jump (see `glide`), on the GPU, so the page isn't restyled every frame. */
  .glide .fill,
  .glide .thumb-track {
    transition: transform 0.3s linear;
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

  /* ---------- shuffle / repeat ---------- */
  /* On: the accent colour and a dot underneath, so on and off differ by more than brightness. */
  .toggle {
    position: relative;
    color: var(--text-2);
  }
  .toggle.on {
    color: var(--accent);
  }
  .toggle.on::after {
    content: "";
    position: absolute;
    left: 50%;
    bottom: 2px;
    width: 4px;
    height: 4px;
    margin-left: -2px;
    border-radius: 50%;
    background: var(--accent);
  }

  /* ---------- seek row: time, line, time (always shown) ---------- */
  .seek-row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .seek-row .time {
    flex: none;
    width: 38px;
    font-size: 11px;
    color: var(--text-2);
    text-align: right;
  }
  .seek-row .time.r {
    text-align: left;
  }
  /* In the row, the line sits in the middle of a 16 px hit area: 4 px thick, 6 px under the pointer. */
  .seek-row .seek {
    position: relative;
    left: auto;
    right: auto;
    top: auto;
    flex: 1;
    height: 16px;
  }
  .seek-row .rail {
    top: 6px;
    height: 4px;
    border-radius: 2px;
    transition:
      height 0.2s cubic-bezier(0.2, 0.8, 0.2, 1),
      top 0.2s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .seek-row .seek:hover .rail,
  .seek-row .seek.active .rail,
  .seek-row .seek:focus-visible .rail {
    top: 5px;
    height: 6px;
  }
  /* The knob shows while a song is loaded, a little bigger under the pointer. */
  .seek-row .thumb {
    top: 8px;
    margin: -6px 0 0 -6px;
    transform: scale(0.7);
  }
  .seek-row .seek:hover .thumb,
  .seek-row .seek.active .thumb,
  .seek-row .seek:focus-visible .thumb {
    transform: scale(1);
  }
  .seek-row .tip {
    top: auto;
    bottom: 18px;
  }

  /* ---------- the song's buttons ---------- */
  .act {
    flex: none;
    width: 28px;
    height: 28px;
    color: var(--text-2);
  }
  .act:hover {
    color: var(--text);
  }
  .act.heart.loved {
    color: var(--accent);
  }
  .icon-btn.sleep {
    width: auto;
    gap: 5px;
    padding: 0 9px;
    border-radius: 14px;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  /* ---------- nothing playing ---------- */
  .idle-btn {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 14px;
    border-radius: inherit;
    color: var(--text-3);
    font-size: 13px;
  }
  .idle-text {
    font-weight: 600;
  }
  .idle-cta {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    border-radius: 999px;
    background: var(--hover);
    color: var(--text-2);
    font-size: 12px;
    font-weight: 600;
  }
  .idle-btn:hover .idle-cta {
    background: var(--accent);
    color: white;
  }

  /* ---------- desktop: the window's top bar ---------- */
  /* Above the page, not over it: nothing scrolls under it, so it needs no live blur; the album's
     colours come through from the ambient light behind the window. */
  .top {
    position: relative;
    z-index: 30;
    flex: none;
    height: 60px;
    display: grid;
    grid-template-columns: 250px minmax(300px, 1fr) auto;
    align-items: center;
    gap: 16px;
    /* The window's own buttons (TitleBar) sit in the last 146 px. */
    padding: 0 146px 0 0;
    background: color-mix(in srgb, var(--ink) 55%, transparent);
    border-bottom: 1px solid var(--line);
  }
  .t-left {
    position: relative;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .top .transport {
    gap: 4px;
  }
  .top .main-btn {
    width: 36px;
    height: 36px;
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.35);
  }
  .top .main-btn.playing {
    box-shadow:
      0 2px 10px rgba(0, 0, 0, 0.35),
      0 0 0 4px color-mix(in srgb, var(--accent) 22%, transparent);
  }
  .top :disabled,
  .bar :disabled {
    opacity: 0.35;
    pointer-events: none;
  }
  .t-right {
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    min-width: 0;
  }
  .top .volume {
    width: 90px;
  }
  /* The middle: the song, like a small display, with its times and seek line along the bottom. */
  .lcd {
    position: relative;
    justify-self: center;
    width: min(100%, 700px);
    height: 50px;
    display: flex;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.045);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  /* The whole display opens Now Playing; everything else in it sits on top. */
  .lcd-open {
    position: absolute;
    inset: 0;
    z-index: 0;
    border-radius: inherit;
  }
  .lcd:not(.idle):hover {
    background: rgba(255, 255, 255, 0.07);
  }
  .lcd .art {
    position: relative;
    z-index: 1;
    width: 50px;
    height: 50px;
    border-radius: 10px 0 0 10px;
    box-shadow: none;
    pointer-events: none;
  }
  .lcd:hover .expand {
    opacity: 1;
  }
  .lcd-body {
    position: relative;
    z-index: 1;
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
    padding: 2px 10px 0 12px;
    /* Clicks between the links fall through to the display (Now Playing). */
    pointer-events: none;
  }
  .lcd-body :is(button, .seek, .meta) {
    pointer-events: auto;
  }
  /* The cover's colours, faintly, from the left. */
  .lcd-glow {
    position: absolute;
    inset: 0;
    background-size: cover;
    background-position: center;
    filter: blur(24px) saturate(1.6);
    opacity: 0.22;
    clip-path: inset(0 round 0 10px 10px 0);
    mask-image: linear-gradient(to right, black, transparent 75%);
    pointer-events: none;
  }
  .lcd-row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .lcd .meta {
    flex: 1;
    /* Only the text itself is a link; the gaps open Now Playing. */
    pointer-events: none;
  }
  .lcd .meta-in {
    gap: 0;
  }
  .lcd .title {
    font-size: 13px;
    line-height: 16px;
    pointer-events: auto;
  }
  .sub {
    display: flex;
    min-width: 0;
    max-width: 100%;
    font-size: 11.5px;
    line-height: 15px;
    color: var(--text-2);
  }
  .sub .artist,
  .album-link {
    flex: 0 1 auto;
    min-width: 0;
    font-size: 11.5px;
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    pointer-events: auto;
  }
  .album-link:hover {
    text-decoration: underline;
  }
  .dash {
    flex: none;
    margin: 0 6px;
    color: var(--text-3);
  }
  .lcd-side {
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
  }
  /* Just "Lossless" here; the bit depth and rate show on hover (the badge's tooltip). */
  .lcd-side :global(.detail) {
    display: none;
  }
  .lcd .fix {
    width: 28px;
    height: 28px;
    margin: 0;
  }
  .lcd .seek-row {
    margin: -1px 0 1px -2px;
    height: 14px;
  }
  .lcd .seek-row .time {
    width: auto;
    min-width: 30px;
    font-size: 10.5px;
  }

  /* The lists of songs before and after drop down from the buttons. */
  .peek.t {
    top: calc(100% + 4px);
    bottom: auto;
  }
  .peek.t::after {
    top: auto;
    bottom: 100%;
    height: 12px;
  }
  .peek.t.prev {
    left: 8px;
    transform: none;
    transform-origin: 30% 0;
  }
  .peek.t.next {
    left: 60px;
    transform: none;
    transform-origin: 45% 0;
  }

  /* ---------- desktop: a bar along the bottom (Settings > Appearance) ---------- */
  .bar {
    position: relative;
    z-index: 30;
    flex: none;
    height: 84px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(320px, 620px) minmax(0, 1fr);
    align-items: center;
    gap: 20px;
    padding: 0 18px 0 12px;
    background: color-mix(in srgb, var(--ink) 62%, transparent);
    border-top: 1px solid var(--line);
  }
  .b-left {
    position: relative;
    height: 60px;
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }
  .b-left .idle-btn {
    justify-content: flex-start;
    padding-left: 6px;
  }
  .b-actions {
    flex: none;
    display: flex;
    align-items: center;
  }
  .b-center {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .b-transport {
    position: relative;
    display: flex;
    justify-content: center;
  }
  .bar .transport {
    gap: 8px;
  }
  .bar .main-btn {
    width: 40px;
    height: 40px;
  }
  .b-right {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    min-width: 0;
  }
  .b-right :global(.detail) {
    display: none;
  }

  @media (max-width: 1080px) {
    .top {
      grid-template-columns: 220px minmax(280px, 1fr) auto;
    }
    .top .volume,
    .bar .volume {
      display: none;
    }
    /* Only the gold mark; the words show on hover. */
    .lcd-side :global(.q),
    .b-right :global(.q) {
      display: none;
    }
  }

  /* Narrower windows: drop the signal chip, then the album name. */
  @media (max-width: 1180px) {
    .path {
      display: none;
    }
  }
  @media (max-width: 960px) {
    .dash,
    .album-link {
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
    .rail,
    .glide .fill,
    .glide .thumb-track {
      transition: none;
    }
  }
</style>

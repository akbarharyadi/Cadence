<script lang="ts">
  import { api, artUrl, formatTime, khz, qualityOf } from "$lib/api";
  import { app, signalPath } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import LyricsView from "./LyricsView.svelte";
  import QualityBadge from "./QualityBadge.svelte";

  const s = $derived(app.status);
  const t = $derived(app.current);
  const album = $derived(app.currentAlbum);
  const art = $derived(artUrl(album?.art));

  let dragging = $state(false);
  let dragValue = $state(0);
  const pos = $derived(dragging ? dragValue : app.position);
  const duration = $derived(s?.duration ?? 0);

  const src = $derived(s?.source);
  const quality = $derived(src ? qualityOf(src.codec, src.sampleRate) : "");
  const detail = $derived(src ? `${src.bits ? `${src.bits}-bit/` : ""}${khz(src.sampleRate)} ${src.codec}` : "");
  const sp = $derived(signalPath(s));

  // Swipe down to close (phones), like Apple Music: the sheet follows the finger and the page
  // behind shows through; past a third of the screen, or a quick flick, it slides away.
  let pull = $state(0);
  let pulling = $state(false);
  let startY = 0;
  let startX = 0;
  let startAt = 0;
  let decided = false;

  function touchStart(e: TouchEvent) {
    const target = e.target as HTMLElement;
    // Leave sliders, buttons' own gestures and scrolled lyrics alone.
    if (target.closest("input, .slider")) return;
    const scroller = target.closest<HTMLElement>(".lyrics, [data-scroll]");
    if (scroller && scroller.scrollTop > 0) return;
    pulling = true;
    decided = false;
    startY = e.touches[0].clientY;
    startX = e.touches[0].clientX;
    startAt = performance.now();
  }
  function touchMove(e: TouchEvent) {
    if (!pulling) return;
    const dy = e.touches[0].clientY - startY;
    const dx = e.touches[0].clientX - startX;
    if (!decided) {
      if (Math.abs(dy) < 8 && Math.abs(dx) < 8) return;
      // Only a mostly-downward drag closes; sideways or upward drags scroll as usual.
      decided = true;
      if (dy <= 0 || Math.abs(dx) > dy) {
        pulling = false;
        return;
      }
    }
    if (e.cancelable) e.preventDefault();
    pull = Math.max(0, dy);
  }
  function touchEnd() {
    if (!pulling) return;
    pulling = false;
    const speed = pull / Math.max(1, performance.now() - startAt);
    if (pull > window.innerHeight / 3 || (pull > 60 && speed > 0.6)) {
      pull = window.innerHeight;
      setTimeout(() => (app.nowPlayingOpen = false), 260);
    } else {
      pull = 0;
    }
  }
  const progress = $derived(Math.min(1, pull / (typeof window === "undefined" ? 800 : window.innerHeight)));
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && (app.nowPlayingOpen = false)} />

{#if pull > 0}<div class="behind" style="opacity: {0.6 * (1 - progress)}" aria-hidden="true"></div>{/if}
<div
  class="np"
  class:with-lyrics={app.lyricsOpen}
  class:pulled={pull > 0}
  class:settling={!pulling}
  role="dialog"
  aria-label="Now Playing"
  tabindex="-1"
  style={pull > 0 ? `transform: translateY(${pull}px); border-radius: ${Math.min(28, pull / 4)}px;` : ""}
  ontouchstart={touchStart}
  ontouchmove={touchMove}
  ontouchend={touchEnd}
  ontouchcancel={touchEnd}
>
  {#if art}<div class="bg" style="background-image: url('{art}')"></div>{/if}
  <div class="shade"></div>

  <div class="top" data-tauri-drag-region>
    <button class="icon-btn light" onclick={() => (app.nowPlayingOpen = false)} aria-label="Close Now Playing" title="Close (Esc)">
      <Icon name="down" size={22} />
    </button>
  </div>

  <div class="stage">
    <div class="left">
      <div class="cover" class:paused={!s?.playing}>
        {#if art}<img src={art} alt="" />{:else}<div class="ph"><Icon name="note" size={80} /></div>{/if}
        {#if t}
          <button class="change-cover" onclick={() => (app.coverEditing = t.albumId)} aria-label="Change cover" title="Change cover">
            <Icon name="edit" size={16} />
          </button>
        {/if}
      </div>

      <div class="info">
        {#if t}
          <div class="title-row">
            <h1>{t.title}</h1>
            {#if app.needsInfo(t)}
              <button class="icon-btn light skip" onclick={() => (app.fixing = t)} aria-label="Fix song info" title="Fix song info">
                <Icon name="sparkle" size={20} />
              </button>
            {/if}
            <button class="icon-btn light skip" onclick={(e) => app.trackMenu(e, [t])} aria-label="More options" title="More options">
              <Icon name="more" size={20} />
            </button>
            <button class="icon-btn light skip" onclick={() => app.setSkipped([t], true)} aria-label="Skip this song from now on" title="Skip this song from now on">
              <Icon name="skip" size={20} />
            </button>
          </div>
          <button class="artist" onclick={() => app.go({ name: "artist", artist: app.artistOf(t.albumArtist) })}>{t.artist}</button>
          <button class="album" onclick={() => app.go({ name: "album", id: t.albumId })}>{t.album}{t.year ? `, ${t.year}` : ""}</button>
        {:else}
          <h1>Nothing playing</h1>
        {/if}

        <div class="seek">
          <input
            type="range"
            class="slider"
            min="0"
            max={Math.max(duration, 1)}
            step="0.1"
            value={pos}
            style="--pct: {duration ? (pos / duration) * 100 : 0}%"
            onpointerdown={() => { dragging = true; dragValue = app.position; }}
            oninput={(e) => (dragValue = +e.currentTarget.value)}
            onchange={(e) => { app.seek(+e.currentTarget.value); dragging = false; }}
            aria-label="Seek"
          />
          <div class="times"><span>{formatTime(pos)}</span><span>-{formatTime(Math.max(0, duration - pos))}</span></div>
        </div>

        <div class="transport">
          <button class="icon-btn light" class:on={s?.repeat !== "off"} onclick={() => api.setRepeat(s?.repeat === "off" ? "all" : s?.repeat === "all" ? "one" : "off")} aria-label="Repeat" title="Repeat">
            <Icon name={s?.repeat === "one" ? "repeatOne" : "repeat"} size={20} />
          </button>
          <button class="icon-btn light big" onclick={() => api.player("prev")} aria-label="Previous"><Icon name="prev" size={30} /></button>
          <button class="icon-btn light huge" onclick={() => api.player("toggle")} aria-label={s?.playing ? "Pause" : "Play"}>
            <Icon name={s?.playing ? "pause" : "play"} size={38} />
          </button>
          <button class="icon-btn light big" onclick={() => api.player("next")} aria-label="Next"><Icon name="next" size={30} /></button>
          <button class="icon-btn light" class:on={app.lyricsOpen} onclick={() => (app.lyricsOpen = !app.lyricsOpen)} aria-label="Lyrics" title="Lyrics">
            <Icon name="lyrics" size={20} />
          </button>
        </div>

        <div class="facts">
          {#if quality}<QualityBadge {quality} {detail} large />{/if}
          {#if sp}
            <button class="path {sp.tier}" onclick={() => app.go({ name: "sound" })} title={sp.steps.join(", ")}>{sp.label}</button>
          {/if}
        </div>
      </div>
    </div>

    {#if app.lyricsOpen && t}
      <div class="right">
        {#key t.id}<LyricsView trackId={t.id} />{/key}
      </div>
    {/if}
  </div>
</div>

<style>
  .title-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }
  .title-row h1 {
    flex: 1;
    min-width: 0;
  }
  .skip {
    flex: none;
    margin-top: 2px;
    opacity: 0.7;
  }
  .skip:hover {
    opacity: 1;
  }
  .cover {
    position: relative;
  }
  .change-cover {
    position: absolute;
    right: 12px;
    bottom: 12px;
    width: 38px;
    height: 38px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: white;
    background: rgba(0, 0, 0, 0.5);
    backdrop-filter: blur(8px);
    opacity: 0;
    transition: opacity 0.18s;
  }
  .cover:hover .change-cover,
  .change-cover:focus-visible {
    opacity: 1;
  }
  @media (hover: none) {
    .change-cover {
      display: none;
    }
  }
  .np {
    position: fixed;
    inset: 0;
    z-index: 40;
    overflow: hidden;
    background: var(--ink);
    color: white;
    animation: rise 0.3s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  /* While being pulled down: follows the finger; on release it glides to its place. */
  .np.pulled {
    animation: none;
    box-shadow: 0 -20px 60px rgba(0, 0, 0, 0.5);
  }
  .np.settling {
    transition:
      transform 0.28s cubic-bezier(0.2, 0.8, 0.2, 1),
      border-radius 0.28s;
  }
  .behind {
    position: fixed;
    inset: 0;
    z-index: 39;
    background: #000;
    pointer-events: none;
  }
  @keyframes rise {
    from {
      transform: translateY(40px);
      opacity: 0;
    }
  }
  .bg {
    position: absolute;
    inset: -15%;
    background-size: cover;
    background-position: center;
    filter: blur(90px) saturate(1.6) brightness(0.7);
    transform: scale(1.1);
    animation: drift 40s ease-in-out infinite alternate;
  }
  @keyframes drift {
    to {
      transform: scale(1.25) rotate(4deg);
    }
  }
  .shade {
    position: absolute;
    inset: 0;
    background: radial-gradient(ellipse at 30% 40%, rgba(0, 0, 0, 0.05), rgba(0, 0, 0, 0.55));
  }
  .top {
    position: absolute;
    inset: 0 0 auto 0;
    height: 72px;
    z-index: 2;
    display: flex;
    align-items: center;
    padding: 20px 24px 0;
  }
  .stage {
    position: relative;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6vw;
    padding: 72px 7vw 48px;
  }
  .left {
    display: flex;
    align-items: center;
    gap: 5vw;
    min-width: 0;
  }
  .with-lyrics .left {
    flex-direction: column;
    align-items: stretch;
    gap: 26px;
    width: min(30vw, 54vh);
    flex: none;
  }
  .right {
    flex: 1;
    min-width: 0;
    height: 100%;
    max-width: 760px;
  }
  .cover {
    width: min(40vw, 66vh);
    aspect-ratio: 1;
    flex: none;
    border-radius: 16px;
    overflow: hidden;
    box-shadow: 0 30px 80px rgba(0, 0, 0, 0.55);
    transition:
      transform 0.45s cubic-bezier(0.2, 0.8, 0.2, 1),
      width 0.4s;
  }
  .with-lyrics .cover {
    width: 100%;
  }
  .cover.paused {
    transform: scale(0.92);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .ph {
    height: 100%;
    display: grid;
    place-items: center;
    background: var(--surface);
    color: var(--text-3);
  }
  .info {
    width: min(460px, 34vw);
    display: flex;
    flex-direction: column;
    align-items: flex-start;
  }
  .with-lyrics .info {
    width: 100%;
  }
  h1 {
    margin: 0 0 8px;
    font-family: var(--display);
    font-size: clamp(26px, 3vw, 44px);
    font-weight: 750;
    font-variation-settings: "wdth" 88;
    line-height: 1.04;
    letter-spacing: -0.03em;
  }
  .with-lyrics h1 {
    font-size: clamp(20px, 1.8vw, 28px);
  }
  .artist {
    font-size: 19px;
    color: rgba(255, 255, 255, 0.85);
    text-align: left;
  }
  .album {
    font-size: 14px;
    color: rgba(255, 255, 255, 0.55);
    margin-top: 3px;
    text-align: left;
  }
  .artist:hover,
  .album:hover {
    text-decoration: underline;
  }
  .seek {
    width: 100%;
    margin-top: 26px;
  }
  .seek .slider {
    width: 100%;
    --fill: white;
  }
  .times {
    display: flex;
    justify-content: space-between;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.6);
    font-variant-numeric: tabular-nums;
    margin-top: 4px;
  }
  .transport {
    width: 100%;
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-top: 14px;
  }
  .icon-btn.light {
    color: rgba(255, 255, 255, 0.85);
  }
  .icon-btn.light:hover {
    color: white;
    background: rgba(255, 255, 255, 0.1);
  }
  .icon-btn.light.on {
    color: white;
    background: rgba(255, 255, 255, 0.16);
  }
  .big {
    width: 54px;
    height: 54px;
  }
  .huge {
    width: 74px;
    height: 74px;
  }
  .facts {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 22px;
  }
  .facts :global(.detail) {
    color: rgba(255, 255, 255, 0.7);
  }
  .path {
    font-size: 12px;
    font-weight: 600;
    padding: 4px 10px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.12);
    color: rgba(255, 255, 255, 0.85);
  }
  .path:hover {
    background: rgba(255, 255, 255, 0.2);
  }
  .path.lossless,
  .path.bitperfect {
    color: var(--gold);
  }
  /* ---------- Phones: one column, like Apple Music on iPhone ---------- */
  @media (max-width: 720px) {
    .top {
      height: 56px;
      padding: 6px 10px 0;
    }
    .stage {
      flex-direction: column;
      justify-content: flex-start;
      align-items: stretch;
      gap: 0;
      padding: 60px 24px 22px;
    }
    .left,
    .with-lyrics .left {
      flex-direction: column;
      align-items: stretch;
      gap: 28px;
      width: 100%;
      flex: 1;
      justify-content: center;
    }
    .cover,
    .with-lyrics .cover {
      width: 100%;
      max-width: 440px;
      align-self: center;
      border-radius: 12px;
    }
    .info,
    .with-lyrics .info {
      width: 100%;
    }
    h1,
    .with-lyrics h1 {
      font-size: 24px;
      margin-bottom: 4px;
      display: -webkit-box;
      -webkit-line-clamp: 2;
      line-clamp: 2;
      -webkit-box-orient: vertical;
      overflow: hidden;
    }
    .artist {
      font-size: 17px;
    }
    .seek {
      margin-top: 20px;
    }
    .transport {
      margin-top: 8px;
    }
    .facts {
      margin-top: 14px;
      justify-content: center;
      width: 100%;
    }

    /* Lyrics open: the words fill the screen, a compact player stays at the bottom. */
    .with-lyrics .stage {
      padding: 56px 16px 16px;
    }
    .with-lyrics .right {
      order: 1;
      flex: 1;
      width: 100%;
      max-width: none;
      height: auto;
      min-height: 0;
    }
    .with-lyrics .left {
      order: 2;
      flex: none;
      gap: 0;
    }
    .with-lyrics .cover,
    .with-lyrics .album,
    .with-lyrics .facts {
      display: none;
    }
    .with-lyrics h1 {
      font-size: 17px;
      -webkit-line-clamp: 1;
      line-clamp: 1;
    }
    .with-lyrics .artist {
      font-size: 14px;
    }
    .with-lyrics .seek {
      margin-top: 10px;
    }
    .with-lyrics .transport {
      margin-top: 2px;
    }
  }
</style>

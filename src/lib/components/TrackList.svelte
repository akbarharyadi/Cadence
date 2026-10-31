<script lang="ts">
  import { artUrl, formatTime, type Track } from "$lib/api";
  import { app, type MenuItem } from "$lib/store.svelte";
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";

  let {
    tracks,
    numbered = false,
    showArt = false,
    showAlbum = false,
    hideArtist = "",
    extraMenu,
    onreorder,
  }: {
    tracks: Track[];
    numbered?: boolean;
    showArt?: boolean;
    showAlbum?: boolean;
    hideArtist?: string;
    /** Extra right-click items for a row (e.g. "Remove from playlist"). */
    extraMenu?: (t: Track, i: number) => MenuItem[];
    /** Rows can be dragged to a new position. */
    onreorder?: (from: number, to: number) => void;
  } = $props();

  let dragFrom = $state<number | null>(null);
  let dropAt = $state<number | null>(null);
  const menu = (e: MouseEvent, i: number) => app.trackMenu(e, tracks, i, extraMenu?.(tracks[i], i) ?? []);

  let selected = $state<string | null>(null);
  const multiDisc = $derived(numbered && new Set(tracks.map((t) => t.discNo ?? 1)).size > 1);

  const isCurrent = (t: Track) => app.current?.id === t.id;
  const play = (i: number) => app.playTracks(tracks, i, true);

  // Long lists only build the rows on screen (plus a margin), so opening Songs with thousands of
  // songs stays quick and page animations don't stall. Rows are all ROW px tall; the space of
  // the rows that aren't built is kept as padding, so the page scrolls and sizes as before.
  const ROW = 54;
  const MARGIN = 12;
  const virtual = $derived(tracks.length > 80 && !multiDisc);
  let listEl = $state<HTMLDivElement>();
  let first = $state(0);
  let last = $state(60);
  $effect(() => {
    const el = listEl;
    if (!virtual || !el) return;
    const scroller = (el.closest(".content") as HTMLElement | null) ?? document.documentElement;
    const count = tracks.length;
    const update = () => {
      const top = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      const from = Math.max(0, Math.floor(-top / ROW) - MARGIN);
      const to = Math.min(count, Math.ceil((scroller.clientHeight - top) / ROW) + MARGIN);
      if (from !== first) first = from;
      if (to !== last) last = Math.max(from, to);
    };
    untrack(update); // reading first/last here mustn't make this effect re-run on every scroll
    scroller.addEventListener("scroll", update, { passive: true });
    const resize = new ResizeObserver(update);
    resize.observe(scroller);
    return () => {
      scroller.removeEventListener("scroll", update);
      resize.disconnect();
    };
  });
  const start = $derived(virtual ? first : 0);
  const shown = $derived(virtual ? tracks.slice(first, last) : tracks);
</script>

<div
  class="list"
  class:numbered
  role="listbox"
  tabindex="-1"
  bind:this={listEl}
  style={virtual ? `padding-top: ${first * ROW}px; padding-bottom: ${Math.max(0, tracks.length - last) * ROW}px` : undefined}
>
  {#each shown as t, k (t.id + (k + start))}
    {@const i = k + start}
    {#if multiDisc && (i === 0 || (tracks[i - 1].discNo ?? 1) !== (t.discNo ?? 1))}
      <div class="disc">Disc {t.discNo ?? 1}</div>
    {/if}
    <div
      class="row"
      class:current={isCurrent(t)}
      class:skipped={app.isSkipped(t)}
      class:selected={selected === t.id}
      class:drop-before={dropAt === i && dragFrom !== null && dragFrom > i}
      class:drop-after={dropAt === i && dragFrom !== null && dragFrom < i}
      draggable={!!onreorder}
      ondragstart={(e) => { dragFrom = i; e.dataTransfer?.setData("text/plain", String(i)); }}
      ondragover={(e) => { if (onreorder && dragFrom !== null) { e.preventDefault(); dropAt = i; } }}
      ondragend={() => { dragFrom = null; dropAt = null; }}
      ondrop={(e) => { e.preventDefault(); if (dragFrom !== null && dragFrom !== i) onreorder?.(dragFrom, i); dragFrom = null; dropAt = null; }}
      role="option"
      aria-selected={selected === t.id}
      tabindex="0"
      onclick={() => (selected = t.id)}
      ondblclick={() => play(i)}
      onkeydown={(e) => e.key === "Enter" && play(i)}
      oncontextmenu={(e) => { selected = t.id; menu(e, i); }}
    >
      <div class="lead">
        {#if showArt}
          <div class="thumb">
            {#if app.albumById.get(t.albumId)?.artSmall}
              <img src={artUrl(app.albumById.get(t.albumId)?.artSmall)} alt="" loading="lazy" />
            {/if}
            {#if isCurrent(t) && app.status?.playing}<div class="eq-anim overlay"><i></i><i></i><i></i></div>{/if}
          </div>
        {:else if isCurrent(t) && app.status?.playing}
          <div class="eq-anim"><i></i><i></i><i></i></div>
        {:else}
          <span class="num">{numbered ? (t.trackNo ?? i + 1) : i + 1}</span>
        {/if}
        <button class="row-play" aria-label="Play {t.title}" onclick={(e) => { e.stopPropagation(); play(i); }}>
          <Icon name="play" size={14} />
        </button>
      </div>
      <div class="main">
        <div class="title">
          {#if app.isSkipped(t)}<span class="skip-mark" title="Skipped when albums, playlists and shuffles play"><Icon name="skip" size={13} /></span>{/if}{t.title}
        </div>
        {#if t.artist !== hideArtist}<div class="sub">{t.artist}</div>{/if}
      </div>
      {#if showAlbum}
        <button class="album" onclick={(e) => { e.stopPropagation(); app.go({ name: "album", id: t.albumId }); }}>{t.album}</button>
      {/if}
      <div class="time">{formatTime(t.duration)}</div>
      <button class="more" aria-label="More options for {t.title}" title="More options" onclick={(e) => { e.stopPropagation(); menu(e, i); }}>
        <Icon name="more" size={16} />
      </button>
    </div>
  {/each}
</div>

<style>
  .list {
    outline: none;
  }
  .disc {
    font-family: var(--display);
    font-weight: 600;
    font-size: 15px;
    color: var(--text-2);
    padding: 20px 12px 8px;
  }
  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 14px;
    height: 54px;
    padding: 0 12px;
    border-radius: var(--r-sm);
    content-visibility: auto;
    contain-intrinsic-size: auto 54px;
    outline: none;
  }
  /* Hairline between rows, hidden next to a highlighted row. */
  .row + .row::before {
    content: "";
    position: absolute;
    top: 0;
    left: 54px;
    right: 12px;
    height: 1px;
    background: var(--line);
  }
  .row:hover::before,
  .row:hover + .row::before,
  .row.selected::before,
  .row.selected + .row::before,
  .row.current::before,
  .row.current + .row::before {
    opacity: 0;
  }
  .row:hover {
    background: var(--hover);
  }
  .row.selected {
    background: var(--active);
  }
  .row.drop-before {
    box-shadow: inset 0 2px 0 var(--accent);
  }
  .row.drop-after {
    box-shadow: inset 0 -2px 0 var(--accent);
  }
  .row:focus-visible {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .row.current {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .row.current .title {
    color: var(--accent);
    font-weight: 600;
  }
  /* Skipped songs stay listed, but quieter. */
  .row.skipped .main,
  .row.skipped .album,
  .row.skipped .thumb {
    opacity: 0.45;
  }
  .skip-mark {
    display: inline-grid;
    vertical-align: -2px;
    margin-right: 6px;
  }
  .lead {
    position: relative;
    width: 28px;
    flex: none;
    display: grid;
    place-items: center;
  }
  .list:has(.thumb) .lead {
    width: 40px;
  }
  .num {
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
    font-size: 13px;
  }
  .thumb {
    width: 40px;
    height: 40px;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface);
    position: relative;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .row-play {
    position: absolute;
    inset: 0;
    display: none;
    place-items: center;
    color: var(--text);
    border-radius: 6px;
  }
  .thumb + .row-play {
    background: rgba(0, 0, 0, 0.5);
    color: white;
  }
  .row:hover .row-play {
    display: grid;
  }
  .row:hover .num,
  .row:hover .lead > .eq-anim {
    visibility: hidden;
  }
  .main {
    flex: 1;
    min-width: 0;
  }
  .title,
  .sub,
  .album {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title {
    font-weight: 500;
  }
  .sub {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .album {
    flex: 0 1 32%;
    min-width: 0;
    text-align: left;
    color: var(--text-2);
    font-size: 13px;
  }
  .album:hover {
    color: var(--text);
    text-decoration: underline;
  }
  .time {
    width: 48px;
    text-align: right;
    color: var(--text-3);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }
  .more {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: var(--text-2);
    opacity: 0;
  }
  .row:hover .more,
  .row.selected .more,
  .more:focus-visible {
    opacity: 1;
  }
  .more:hover {
    background: var(--active);
    color: var(--text);
  }

  /* Little "now playing" bars. */
  .eq-anim {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 14px;
  }
  .eq-anim.overlay {
    position: absolute;
    inset: 0;
    height: auto;
    padding: 12px 11px;
    background: rgba(0, 0, 0, 0.45);
  }
  /* Scaled, not resized, so the page isn't laid out again on every frame. */
  .eq-anim i {
    width: 3px;
    align-self: stretch;
    background: var(--accent);
    border-radius: 1px;
    transform-origin: bottom;
    animation: bounce 0.9s ease-in-out infinite;
  }
  .eq-anim.overlay i {
    background: white;
  }
  .eq-anim i:nth-child(2) {
    animation-delay: -0.3s;
  }
  .eq-anim i:nth-child(3) {
    animation-delay: -0.6s;
  }
  @keyframes bounce {
    0%,
    100% {
      transform: scaleY(0.3);
    }
    50% {
      transform: scaleY(1);
    }
  }
  /* Phones: the album column would squeeze the titles; it's still one tap away in the menu. */
  @media (max-width: 720px) {
    .album {
      display: none;
    }
    .row {
      padding: 0 6px;
    }
  }
</style>

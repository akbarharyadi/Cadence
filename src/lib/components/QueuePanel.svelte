<script lang="ts">
  import { api, artUrl, formatTime, type Track } from "#lib/api.js";
  import { app } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";

  // Each piece of the status on its own: the status object is replaced four times a second, and a
  // narrow value that didn't change wakes nothing up.
  const repeat = $derived(app.status?.repeat);
  // The lists depend on the queue and the current index only (not the whole status, which is
  // replaced four times a second), and only the part of the queue that is listed is walked.
  const queue = $derived(app.status?.queue);
  const index = $derived(app.status?.index ?? null);
  const SHOWN = 200;
  /** Songs waiting after the current one: the real number, whatever is listed. */
  const upNextTotal = $derived(queue && index != null ? Math.max(0, queue.length - index - 1) : 0);
  const upNext = $derived.by(() => {
    if (!queue || index == null) return [];
    const out: { track: Track | undefined; i: number }[] = [];
    for (let i = index + 1; i < queue.length && out.length < SHOWN; i++) {
      const track = app.trackById.get(queue[i]);
      if (track) out.push({ track, i });
    }
    return out;
  });
  // Small values only (cheap to follow at four updates a second).
  const stopMark = $derived(app.status?.stopMark ?? null);
  const autoFrom = $derived(app.status?.autoplayFrom ?? null);
  const albumShuffle = $derived(!!app.status?.albumShuffle);

  /** The row's menu from the keyboard (the menu key, or Shift+F10). */
  function menuKey(e: KeyboardEvent, i: number) {
    if (e.key !== "ContextMenu" && !(e.shiftKey && e.key === "F10")) return false;
    e.preventDefault();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    app.queueMenu(new MouseEvent("contextmenu", { clientX: r.left + 24, clientY: r.bottom }), i);
    return true;
  }

  const history = $derived.by(() => {
    if (!queue || index == null) return [];
    const out: { track: Track | undefined; i: number }[] = [];
    for (let i = Math.max(0, index - 20); i < index; i++) {
      const track = app.trackById.get(queue[i]);
      if (track) out.push({ track, i });
    }
    return out;
  });

  // Dragging a waiting song to another place (queue positions).
  let dragFrom = $state<number | null>(null);
  let dropAt = $state<number | null>(null);
  function drop(e: DragEvent, to: number) {
    e.preventDefault();
    if (dragFrom !== null && dragFrom !== to) api.moveInQueue(dragFrom, to);
    dragFrom = dropAt = null;
  }

  // "Jump to current": shown only while the playing song's row is scrolled out of the list. An
  // IntersectionObserver on that one row tells (no scroll listener, nothing per scroll event).
  let scrollEl = $state<HTMLDivElement>();
  let currentEl = $state<HTMLElement>();
  let away = $state(false);
  $effect(() => {
    const root = scrollEl;
    const row = currentEl;
    away = false;
    if (!root || !row || typeof IntersectionObserver === "undefined") return;
    const watch = new IntersectionObserver(
      (entries) => {
        const last = entries[entries.length - 1];
        if (last) away = !last.isIntersecting;
      },
      { root },
    );
    watch.observe(row);
    return () => watch.disconnect();
  });

  /** Scrolls the list so the playing song is at its top (the "Now Playing" label stays in view). */
  function toCurrent(smooth: boolean) {
    const root = scrollEl;
    const row = currentEl;
    if (!root || !row) return;
    const top = row.getBoundingClientRect().top - root.getBoundingClientRect().top + root.scrollTop - 40;
    root.scrollTo({ top: Math.max(0, top), behavior: smooth && !matchMedia("(prefers-reduced-motion: reduce)").matches ? "smooth" : "auto" });
  }

  // The panel opens at the playing song, not wherever the list was left.
  let started = false;
  $effect(() => {
    if (started || !currentEl) return;
    started = true;
    toCurrent(false);
  });
</script>

<aside class="queue">
  <header>
    <h3>Up Next</h3>
    <div class="head-actions">
      <button class="icon-btn" onclick={() => app.saveQueueAsPlaylist()} disabled={!queue?.length} aria-label="Save the queue as a playlist" title="Save the queue as a playlist">
        <Icon name="playlist" size={16} />
      </button>
      <button class="icon-btn" onclick={() => app.clearUpNext()} disabled={!upNextTotal} aria-label="Clear Up Next" title="Clear Up Next (the playing song stays)">
        <Icon name="trash" size={16} />
      </button>
      <button class="icon-btn" onclick={() => (app.queueOpen = false)} aria-label="Close"><Icon name="close" size={16} /></button>
    </div>
  </header>
  <div class="scroll" bind:this={scrollEl}>
    {#if app.current}
      <div class="label">Now Playing</div>
      <div
        class="item current"
        bind:this={currentEl}
        role="button"
        tabindex="0"
        oncontextmenu={(e) => index != null && app.queueMenu(e, index)}
        onkeydown={(e) => index != null && menuKey(e, index)}
      >
        <img src={artUrl(app.currentAlbum?.artSmall)} alt="" />
        <div class="meta"><div class="t">{app.current.title}</div><div class="a">{app.current.artist}</div></div>
        {#if stopMark != null && stopMark === index}<span class="stop-mark" title="Playback stops after this song"><Icon name="moon" size={12} />Stop after</span>{/if}
      </div>
    {/if}

    {#if upNext.length}
      <div class="label">
        Next, {repeat === "all" ? "then repeating" : `${upNextTotal} ${upNextTotal === 1 ? "song" : "songs"}`}{upNextTotal > upNext.length && upNext.length >= SHOWN ? ` (first ${SHOWN} shown)` : ""}{albumShuffle ? ", shuffling albums" : ""}
      </div>
      {#each upNext as { track, i } (i)}
        {#if i === autoFrom}
          <div class="label auto">
            <span>Autoplay: songs like these</span>
            <button class="link" onclick={() => app.clearAutoplay()} title="Take Autoplay’s songs out of Up Next">Clear</button>
          </div>
        {/if}
        <div
          class="item"
          class:drop-before={dropAt === i && dragFrom !== null && dragFrom > i}
          class:drop-after={dropAt === i && dragFrom !== null && dragFrom < i}
          role="button"
          tabindex="0"
          draggable="true"
          ondragstart={(e) => { dragFrom = i; e.dataTransfer?.setData("text/plain", String(i)); }}
          ondragover={(e) => { if (dragFrom !== null) { e.preventDefault(); dropAt = i; } }}
          ondragend={() => (dragFrom = dropAt = null)}
          ondrop={(e) => drop(e, i)}
          ondblclick={() => api.jump(i)}
          oncontextmenu={(e) => app.queueMenu(e, i)}
          onkeydown={(e) => menuKey(e, i) || (e.key === "Enter" && api.jump(i))}
        >
          <img src={artUrl(app.albumById.get(track!.albumId)?.artSmall)} alt="" loading="lazy" />
          <div class="meta"><div class="t">{track!.title}</div><div class="a">{track!.artist}</div></div>
          {#if stopMark === i}<span class="stop-mark" title="Playback stops after this song"><Icon name="moon" size={12} />Stop after</span>{/if}
          <span class="d">{formatTime(track!.duration)}</span>
          <button class="rm" aria-label="Remove from queue" onclick={() => app.removeFromQueue(i)}><Icon name="close" size={14} /></button>
        </div>
      {/each}
    {:else}
      <p class="empty">Nothing up next. Right-click a song → <b>Play Next</b> or <b>Add to Queue</b>.</p>
    {/if}

    {#if history.length}
      <div class="label">Played</div>
      {#each history as { track, i } (i)}
        <div class="item played" role="button" tabindex="0" ondblclick={() => api.jump(i)} onkeydown={(e) => e.key === "Enter" && api.jump(i)}>
          <img src={artUrl(app.albumById.get(track!.albumId)?.artSmall)} alt="" loading="lazy" />
          <div class="meta"><div class="t">{track!.title}</div><div class="a">{track!.artist}</div></div>
        </div>
      {/each}
    {/if}
  </div>
  {#if away}
    <button class="jump" onclick={() => toCurrent(true)}><Icon name="back" size={14} />Jump to current</button>
  {/if}
</aside>

<style>
  .queue {
    width: 320px;
    flex: none;
    border-left: 1px solid var(--line);
    background: rgba(16, 17, 26, 0.6);
    backdrop-filter: blur(20px);
    padding-bottom: 12px;
    display: flex;
    flex-direction: column;
    animation: slide 0.18s ease-out;
    position: relative;
  }
  /* A pill over the bottom of the list (opacity and transform only). */
  .jump {
    position: absolute;
    left: 50%;
    bottom: 20px;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 14px 0 10px;
    border-radius: 999px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
    background: rgba(36, 39, 61, 0.97);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 8px 24px rgba(0, 0, 0, 0.45);
    animation: jump-in 0.18s ease-out;
  }
  .jump :global(svg) {
    transform: rotate(90deg);
    color: var(--accent);
  }
  .jump:hover {
    background: var(--surface-2);
  }
  @keyframes jump-in {
    from {
      opacity: 0;
      transform: translate(-50%, 8px);
    }
  }
  @keyframes slide {
    from {
      transform: translateX(24px);
      opacity: 0;
    }
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 44px 12px 6px 18px;
  }
  h3 {
    margin: 0;
    font-size: 20px;
    font-family: var(--display);
    letter-spacing: -0.015em;
  }
  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 0 8px 16px;
  }
  .label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
    padding: 14px 10px 6px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: 8px;
    outline: none;
  }
  .item:hover {
    background: var(--hover);
  }
  .label.auto {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .label.auto .link {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
  }
  .label.auto .link:hover {
    text-decoration: underline;
  }
  /* "Stop after this song" on a row. */
  .stop-mark {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    padding: 2px 7px;
    border-radius: 999px;
    background: var(--active);
  }
  .item.drop-before {
    box-shadow: inset 0 2px 0 var(--accent);
  }
  .item.drop-after {
    box-shadow: inset 0 -2px 0 var(--accent);
  }
  .head-actions {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .head-actions .icon-btn:disabled {
    opacity: 0.35;
  }
  .item.current .t {
    color: var(--accent);
  }
  .item.played {
    opacity: 0.55;
  }
  img {
    width: 40px;
    height: 40px;
    border-radius: 5px;
    object-fit: cover;
    background: var(--surface);
    flex: none;
  }
  .meta {
    flex: 1;
    min-width: 0;
  }
  .t,
  .a {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .a {
    font-size: 12px;
    color: var(--text-2);
  }
  .d {
    font-size: 12px;
    color: var(--text-3);
  }
  .rm {
    display: none;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    place-items: center;
    color: var(--text-2);
  }
  .item:hover .rm {
    display: grid;
  }
  .item:hover .d {
    display: none;
  }
  .rm:hover {
    background: var(--active);
    color: var(--text);
  }
  .empty {
    color: var(--text-2);
    padding: 8px 10px;
    font-size: 13px;
  }
</style>

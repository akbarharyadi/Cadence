<script lang="ts">
  import { api, artUrl, formatTime } from "#lib/api.js";
  import { app } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";

  const s = $derived(app.status);
  const upNext = $derived.by(() => {
    if (!s || s.index == null) return [];
    return s.queue
      .map((id, i) => ({ track: app.trackById.get(id), i }))
      .filter((x) => x.i > s.index! && x.track)
      .slice(0, 200);
  });
  const history = $derived.by(() => {
    if (!s || s.index == null) return [];
    return s.queue.slice(0, s.index).map((id, i) => ({ track: app.trackById.get(id), i })).filter((x) => x.track).slice(-20);
  });

  // Dragging a waiting song to another place (queue positions).
  let dragFrom = $state<number | null>(null);
  let dropAt = $state<number | null>(null);
  function drop(e: DragEvent, to: number) {
    e.preventDefault();
    if (dragFrom !== null && dragFrom !== to) api.moveInQueue(dragFrom, to);
    dragFrom = dropAt = null;
  }
</script>

<aside class="queue">
  <header>
    <h3>Up Next</h3>
    <div class="head-actions">
      <button class="icon-btn" onclick={() => app.saveQueueAsPlaylist()} disabled={!s?.queue.length} aria-label="Save the queue as a playlist" title="Save the queue as a playlist">
        <Icon name="playlist" size={16} />
      </button>
      <button class="icon-btn" onclick={() => app.clearUpNext()} disabled={!upNext.length} aria-label="Clear Up Next" title="Clear Up Next (the playing song stays)">
        <Icon name="trash" size={16} />
      </button>
      <button class="icon-btn" onclick={() => (app.queueOpen = false)} aria-label="Close"><Icon name="close" size={16} /></button>
    </div>
  </header>
  <div class="scroll">
    {#if app.current}
      <div class="label">Now Playing</div>
      <div class="item current">
        <img src={artUrl(app.currentAlbum?.artSmall)} alt="" />
        <div class="meta"><div class="t">{app.current.title}</div><div class="a">{app.current.artist}</div></div>
      </div>
    {/if}

    {#if upNext.length}
      <div class="label">Next, {s?.repeat === "all" ? "then repeating" : `${upNext.length} ${upNext.length === 1 ? "song" : "songs"}`}</div>
      {#each upNext as { track, i } (i)}
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
          onkeydown={(e) => e.key === "Enter" && api.jump(i)}
        >
          <img src={artUrl(app.albumById.get(track!.albumId)?.artSmall)} alt="" loading="lazy" />
          <div class="meta"><div class="t">{track!.title}</div><div class="a">{track!.artist}</div></div>
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

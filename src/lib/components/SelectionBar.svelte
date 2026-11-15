<script lang="ts">
  import Icon from "./Icon.svelte";

  /** The bar that floats over a song list while several songs are selected (or, on phones, while
   *  the list is in selection mode): how many, the most used actions, and the rest in "More". */
  let {
    count,
    onplay,
    onqueue,
    onplaylist,
    onremove,
    onmore,
    onclear,
  }: {
    count: number;
    onplay: () => void;
    onqueue: () => void;
    onplaylist: () => void;
    /** Only in a user playlist. */
    onremove?: () => void;
    onmore: (e: MouseEvent) => void;
    onclear: () => void;
  } = $props();

  // Moved to the end of the page, so no transformed or clipped ancestor (a page animating in)
  // changes where a fixed bar sits. It is the only node of its block, so Svelte still removes it.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy: () => node.remove() };
  }
</script>

<div
  class="selbar"
  use:portal
  role="toolbar"
  aria-label="Selected songs"
  tabindex="-1"
  onkeydown={(e) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    onclear();
  }}
>
  <button class="icon-btn" aria-label="Clear selection" title="Clear selection (Esc)" onclick={onclear}><Icon name="close" size={16} /></button>
  <span class="count" aria-live="polite">{count.toLocaleString()} selected</span>
  <button class="pill" onclick={onplay} disabled={!count}><Icon name="play" size={14} />Play</button>
  <button class="icon-btn" aria-label="Add to Queue" title="Add to Queue" onclick={onqueue} disabled={!count}><Icon name="queue" size={18} /></button>
  <button class="icon-btn" aria-label="Add to Playlist…" title="Add to Playlist…" onclick={onplaylist} disabled={!count}><Icon name="playlist" size={18} /></button>
  {#if onremove}
    <button class="icon-btn" aria-label="Remove from this playlist" title="Remove from this playlist (Delete)" onclick={onremove} disabled={!count}><Icon name="trash" size={17} /></button>
  {/if}
  <button class="icon-btn" aria-label="More actions" title="More actions" onclick={(e) => onmore(e)} disabled={!count}><Icon name="more" size={18} /></button>
</div>

<style>
  /* Desktop: centred near the bottom, above where toasts appear (so "Added to queue" doesn't hide it).
     Only opacity and transform animate: the list under it never moves. */
  .selbar {
    position: fixed;
    left: 50%;
    bottom: 84px;
    z-index: 36;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px 6px 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel) 94%, transparent);
    backdrop-filter: blur(20px);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 12px 32px rgba(0, 0, 0, 0.45);
    transform: translateX(-50%);
    animation: selbar-in 0.16s ease-out;
    outline: none;
  }
  .count {
    padding: 0 10px 0 2px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .pill {
    height: 32px;
    padding: 0 14px;
  }
  .icon-btn:disabled {
    opacity: 0.45;
  }
  /* A player along the bottom: above it (and above the toasts there). */
  :global(html.player-bottom) .selbar {
    bottom: 164px;
  }
  @keyframes selbar-in {
    from {
      opacity: 0;
      transform: translate(-50%, 8px);
    }
  }
  /* Phones: across the top, like Android's selection bar; the player and the tab bar below stay free. */
  @media (max-width: 720px) {
    .selbar {
      left: 8px;
      right: 8px;
      bottom: auto;
      top: calc(env(safe-area-inset-top) + 6px);
      transform: none;
      animation-name: selbar-down;
    }
    .count {
      flex: 1;
    }
  }
  :global(html.phone) .selbar {
    left: 8px;
    right: 8px;
    bottom: auto;
    top: calc(env(safe-area-inset-top) + 6px);
    transform: none;
    animation-name: selbar-down;
  }
  :global(html.phone) .count {
    flex: 1;
  }
  @keyframes selbar-down {
    from {
      opacity: 0;
      transform: translateY(-8px);
    }
  }
</style>

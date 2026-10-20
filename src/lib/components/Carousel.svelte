<script lang="ts">
  // A horizontal row of cards with arrow buttons; scrolls a page at a time and snaps to cards.
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  let { title, actions, children, gap = 22 }: { title: string; actions?: Snippet; children: Snippet; gap?: number } = $props();

  let track = $state<HTMLDivElement>();
  let atStart = $state(true);
  let atEnd = $state(true);

  function update() {
    if (!track) return;
    atStart = track.scrollLeft <= 2;
    atEnd = track.scrollLeft + track.clientWidth >= track.scrollWidth - 2;
  }

  function page(dir: 1 | -1) {
    if (!track) return;
    track.scrollBy({ left: dir * track.clientWidth * 0.9, behavior: "smooth" });
  }

  $effect(() => {
    if (!track) return;
    update();
    const ro = new ResizeObserver(update);
    ro.observe(track);
    for (const c of track.children) ro.observe(c);
    return () => ro.disconnect();
  });
</script>

<section class="carousel">
  <div class="head">
    <h2 class="section-title">{title}</h2>
    <div class="right">
      {@render actions?.()}
      {#if !(atStart && atEnd)}
        <div class="arrows">
          <button class="icon-btn" onclick={() => page(-1)} disabled={atStart} aria-label="Scroll {title} back"><Icon name="back" size={16} /></button>
          <button class="icon-btn next" onclick={() => page(1)} disabled={atEnd} aria-label="Scroll {title} forward"><Icon name="back" size={16} /></button>
        </div>
      {/if}
    </div>
  </div>
  <div class="track" bind:this={track} onscroll={update} style="--gap: {gap}px">
    {@render children()}
  </div>
</section>

<style>
  .carousel {
    margin-bottom: 40px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  .right {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .arrows {
    display: flex;
    gap: 4px;
  }
  .arrows .icon-btn {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    background: var(--hover);
  }
  .arrows .icon-btn:disabled {
    opacity: 0.3;
  }
  .next :global(svg) {
    transform: scaleX(-1);
  }
  .track {
    display: flex;
    gap: var(--gap);
    overflow-x: auto;
    scroll-snap-type: x mandatory;
    scroll-padding: 0 4px;
    padding: 16px 4px 12px;
    margin: 0 -4px;
    scrollbar-width: none;
  }
  .track::-webkit-scrollbar {
    display: none;
  }
  /* Cards keep their size and line up on a card edge after each scroll. */
  .track > :global(*) {
    flex: none;
    scroll-snap-align: start;
  }
  @media (prefers-reduced-motion: reduce) {
    .track {
      scroll-behavior: auto;
    }
  }
  /* Phones: swipe instead of arrow buttons. */
  @media (max-width: 720px) {
    .arrows {
      display: none;
    }
  }
</style>

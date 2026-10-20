<script lang="ts">
  import Icon from "./Icon.svelte";

  let { quality, detail = "", large = false }: { quality: string; detail?: string; large?: boolean } = $props();
  const lossless = $derived(quality.includes("Lossless"));
</script>

<!-- Lossless is always champagne gold, so it reads at a glance and never clashes with the album accent. -->
<span class="badge" class:large class:lossless title={detail ? `${quality}, ${detail}` : quality}>
  {#if lossless}<Icon name="lossless" size={large ? 16 : 13} />{/if}
  <span class="q">{quality}</span>
  {#if detail}<span class="detail">{detail}</span>{/if}
</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-2);
    white-space: nowrap;
  }
  .badge.lossless .q,
  .badge.lossless :global(svg) {
    color: var(--gold);
  }
  .large {
    font-size: 13px;
  }
  .detail {
    font-weight: 500;
    color: var(--text-3);
  }
  .large .detail {
    color: var(--text-2);
  }
</style>

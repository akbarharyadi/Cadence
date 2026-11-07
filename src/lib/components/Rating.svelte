<script lang="ts">
  import Icon from "./Icon.svelte";

  /** Five stars. Clicking a star rates; clicking the rating you already gave clears it (the
   * parent decides). Hovering previews. */
  let { value = 0, size = 14, onrate, label = "Rating" }: { value?: number; size?: number; onrate: (stars: number) => void; label?: string } = $props();

  let hover = $state(0);
  const shown = $derived(hover || value);
</script>

<span class="stars" role="group" aria-label={label} onpointerleave={() => (hover = 0)}>
  {#each [1, 2, 3, 4, 5] as n}
    <button
      type="button"
      aria-pressed={value === n}
      aria-label="{n} {n === 1 ? 'star' : 'stars'}"
      title={value === n ? "Clear rating" : `${n} ${n === 1 ? "star" : "stars"}`}
      class:on={n <= shown}
      onpointerenter={() => (hover = n)}
      onclick={(e) => {
        e.stopPropagation();
        onrate(n);
      }}
    >
      <Icon name={n <= shown ? "starFill" : "star"} {size} />
    </button>
  {/each}
</span>

<style>
  .stars {
    display: inline-flex;
    align-items: center;
    gap: 1px;
  }
  button {
    display: grid;
    place-items: center;
    padding: 2px;
    color: var(--text-3);
    border-radius: 4px;
  }
  button.on {
    color: var(--accent);
  }
  button:focus-visible {
    outline: 2px solid var(--accent);
  }
</style>

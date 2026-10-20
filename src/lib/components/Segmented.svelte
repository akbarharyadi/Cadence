<script lang="ts" generics="T extends string">
  let { options, value, onchange, label = "" }: { options: { value: T; label: string }[]; value: T; onchange: (v: T) => void; label?: string } =
    $props();
</script>

<div class="seg" role="radiogroup" aria-label={label}>
  {#each options as o (o.value)}
    <button role="radio" aria-checked={value === o.value} class:active={value === o.value} onclick={() => onchange(o.value)}>{o.label}</button>
  {/each}
</div>

<style>
  .seg {
    display: inline-flex;
    padding: 2px;
    border-radius: 999px;
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  button {
    padding: 6px 15px;
    border-radius: 999px;
    font-size: 13px;
    color: var(--text-2);
    transition: background 0.15s, color 0.15s;
  }
  button:hover {
    color: var(--text);
  }
  button.active {
    background: var(--text);
    color: var(--ink);
    font-weight: 600;
  }
</style>

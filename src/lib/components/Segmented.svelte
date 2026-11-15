<script lang="ts" generics="T extends string">
  let { options, value, onchange, label = "" }: { options: { value: T; label: string }[]; value: T; onchange: (v: T) => void; label?: string } =
    $props();

  let group = $state<HTMLDivElement>();

  /** A radio group's keys: the arrows (and Home / End) move to the next choice and pick it. They
   *  stay here: the window's arrow keys (seek) don't also run. */
  function onKey(e: KeyboardEvent) {
    const i = options.findIndex((o) => o.value === value);
    let next = -1;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (i + 1) % options.length;
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (i - 1 + options.length) % options.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = options.length - 1;
    else return;
    e.preventDefault();
    e.stopPropagation();
    if (next < 0 || !options[next]) return;
    onchange(options[next].value);
    group?.querySelectorAll<HTMLButtonElement>("[role=radio]")[next]?.focus();
  }
</script>

<div class="seg" role="radiogroup" aria-label={label} bind:this={group} onkeydown={onKey} tabindex="-1">
  {#each options as o, i (o.value)}
    {@const checked = value === o.value}
    <button
      role="radio"
      aria-checked={checked}
      tabindex={checked || (i === 0 && !options.some((x) => x.value === value)) ? 0 : -1}
      class:active={checked}
      onclick={() => onchange(o.value)}>{o.label}</button
    >
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
  .seg:focus {
    outline: none;
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

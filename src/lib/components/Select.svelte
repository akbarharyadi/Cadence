<script lang="ts" generics="T extends string">
  // A dropdown that matches the app (the native <select> popup ignores dark styling on Windows).
  import Icon from "./Icon.svelte";

  let {
    options,
    value,
    onchange,
    label,
    width = "auto",
  }: { options: { value: T; label: string }[]; value: T; onchange: (v: T) => void; label: string; width?: string } = $props();

  let open = $state(false);
  let active = $state(0);
  let button: HTMLButtonElement;
  let list = $state<HTMLDivElement>();
  let placeUp = $state(false);

  const current = $derived(options.find((o) => o.value === value));

  function toggle() {
    if (open) return close();
    active = Math.max(0, options.findIndex((o) => o.value === value));
    const r = button.getBoundingClientRect();
    placeUp = window.innerHeight - r.bottom < Math.min(320, options.length * 34 + 16);
    open = true;
    queueMicrotask(() => list?.querySelectorAll<HTMLElement>("[role=option]")[active]?.scrollIntoView({ block: "nearest" }));
  }

  function close() {
    open = false;
    button?.focus();
  }

  function pick(v: T) {
    onchange(v);
    close();
  }

  function onKey(e: KeyboardEvent) {
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        toggle();
      }
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      active = Math.min(options.length - 1, active + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      active = Math.max(0, active - 1);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      pick(options[active].value);
    }
  }
</script>

<div class="select" style="width: {width}">
  <button
    bind:this={button}
    class="trigger"
    class:open
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label={label}
    onclick={toggle}
    onkeydown={onKey}
  >
    <span class="value">{current?.label ?? ""}</span>
    <Icon name="down" size={14} />
  </button>
  {#if open}
    <div class="backdrop" role="presentation" onmousedown={() => (open = false)}></div>
    <div class="list" class:up={placeUp} role="listbox" aria-label={label} bind:this={list}>
      {#each options as o, i (o.value)}
        <div
          role="option"
          tabindex="-1"
          aria-selected={o.value === value}
          class:active={i === active}
          class:selected={o.value === value}
          onmouseenter={() => (active = i)}
          onclick={() => pick(o.value)}
          onkeydown={onKey}
        >
          <span class="check">{#if o.value === value}<svg width="12" height="12" viewBox="0 0 12 12"><path d="M2 6.5 5 9l5-6" stroke="currentColor" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round" /></svg>{/if}</span>
          {o.label}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .select {
    position: relative;
    display: inline-block;
  }
  .trigger {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    height: 34px;
    padding: 0 12px 0 14px;
    border-radius: var(--r-sm);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    font-size: 13px;
    color: var(--text);
    transition: background 0.15s;
  }
  .trigger:hover,
  .trigger.open {
    background: var(--active);
  }
  .trigger :global(svg) {
    color: var(--text-2);
    transition: transform 0.15s;
  }
  .trigger.open :global(svg) {
    transform: rotate(180deg);
  }
  .value {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
  }
  .list {
    position: absolute;
    z-index: 61;
    top: calc(100% + 6px);
    left: 0;
    min-width: 100%;
    max-height: 320px;
    overflow-y: auto;
    padding: 5px;
    border-radius: var(--r-md);
    background: rgba(32, 34, 54, 0.96);
    backdrop-filter: blur(24px) saturate(1.5);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 16px 40px rgba(0, 0, 0, 0.5);
    animation: drop 0.12s ease-out;
  }
  .list.up {
    top: auto;
    bottom: calc(100% + 6px);
  }
  [role="option"] {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 12px 0 8px;
    border-radius: var(--r-sm);
    white-space: nowrap;
    font-size: 13px;
    color: var(--text);
  }
  [role="option"].active {
    background: var(--active);
  }
  [role="option"].selected {
    color: var(--accent);
    font-weight: 600;
  }
  .check {
    width: 14px;
    display: grid;
    place-items: center;
  }
  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
</style>

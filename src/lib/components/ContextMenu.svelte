<script lang="ts">
  import { app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";

  let menuEl = $state<HTMLDivElement>();
  let pos = $state({ x: 0, y: 0 });

  // Keep the menu on screen.
  $effect(() => {
    const m = app.menu;
    if (!m || !menuEl) return;
    const r = menuEl.getBoundingClientRect();
    pos = { x: Math.min(m.x, window.innerWidth - r.width - 8), y: Math.min(m.y, window.innerHeight - r.height - 8) };
  });

  const close = () => (app.menu = null);
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && close()} onblur={close} />

{#if app.menu}
  <div class="backdrop" role="presentation" onmousedown={close} oncontextmenu={(e) => { e.preventDefault(); close(); }}></div>
  <div class="menu" bind:this={menuEl} style="left: {pos.x}px; top: {pos.y}px" role="menu">
    {#each app.menu.items as item}
      <button role="menuitem" onclick={() => { close(); item.action(); }}>
        {#if item.icon}<Icon name={item.icon} size={15} />{/if}
        {item.label}
      </button>
    {/each}
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
  .menu {
    position: fixed;
    z-index: 91;
    min-width: 200px;
    padding: 5px;
    border-radius: var(--r-md);
    background: rgba(32, 34, 54, 0.96);
    backdrop-filter: blur(24px) saturate(1.5);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 16px 40px rgba(0, 0, 0, 0.5);
    animation: pop 0.12s ease-out;
  }
  button {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 10px;
    height: 32px;
    padding: 0 10px;
    border-radius: var(--r-sm);
    text-align: left;
    color: var(--text);
    font-size: 13px;
  }
  button :global(svg) {
    color: var(--text-2);
  }
  button:hover,
  button:focus-visible {
    background: var(--active);
    outline: none;
  }
  button:hover :global(svg) {
    color: var(--accent);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
  }
</style>

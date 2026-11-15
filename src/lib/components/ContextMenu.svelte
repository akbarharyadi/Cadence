<script lang="ts">
  import { app } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";
  import Rating from "./Rating.svelte";

  const EDGE = 8;
  let menuEl = $state<HTMLDivElement>();
  let pos = $state({ x: 0, y: 0 });
  /** Hidden until it's placed, so a menu opened again doesn't show for a frame where the last one was. */
  let placed = $state(false);
  /** What had the keyboard before the menu opened: it gets it back when the menu closes. */
  let opener: HTMLElement | null = null;
  let shownMenu: unknown = null;

  // Keep the menu on screen (on all four sides; a menu taller than the window scrolls), and give
  // its first item the keyboard.
  $effect(() => {
    const m = app.menu;
    if (!m || !menuEl) return;
    const first = shownMenu === null;
    shownMenu = m;
    if (first) {
      const active = document.activeElement;
      opener = active instanceof HTMLElement && active !== document.body && !menuEl.contains(active) ? active : null;
    }
    const r = menuEl.getBoundingClientRect();
    const maxX = window.innerWidth - r.width - EDGE;
    const maxY = window.innerHeight - r.height - EDGE;
    // Not enough room below the pointer: open upwards from it, else as low as fits.
    const y = m.y <= maxY ? m.y : m.y - r.height >= EDGE ? m.y - r.height : maxY;
    pos = { x: Math.max(EDGE, Math.min(m.x, maxX)), y: Math.max(EDGE, y) };
    placed = true;
    if (first) {
      // (A hidden element can't take the keyboard: show it now, not when the page next updates.)
      menuEl.style.visibility = "visible";
      items()[0]?.focus({ preventScroll: true });
    }
  });

  // Closed: the keyboard goes back where it was (unless what the menu did took it elsewhere,
  // e.g. a dialog that opened).
  $effect(() => {
    if (app.menu) return;
    placed = false;
    if (shownMenu === null) return;
    shownMenu = null;
    const back = opener;
    opener = null;
    const active = document.activeElement;
    if (back?.isConnected && (!active || active === document.body)) back.focus({ preventScroll: true });
  });

  const close = () => (app.menu = null);

  /** The menu's buttons (the rating's stars too), in order. */
  const items = () => [...(menuEl?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];

  function onMenuKey(e: KeyboardEvent) {
    const list = items();
    if (!list.length) return;
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    let next = -1;
    if (e.key === "ArrowDown") next = i < 0 ? 0 : (i + 1) % list.length;
    else if (e.key === "ArrowUp") next = i < 0 ? list.length - 1 : (i - 1 + list.length) % list.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = list.length - 1;
    else if (e.key === "Tab") {
      e.preventDefault();
      close();
      return;
    } else return;
    e.preventDefault();
    list[next]?.focus();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key !== "Escape" || !app.menu || e.defaultPrevented) return;
    // Used here: a dialog under the menu stays open.
    e.preventDefault();
    close();
  }
</script>

<svelte:window onkeydown={onKey} onblur={close} />

{#if app.menu}
  <div class="backdrop" role="presentation" onmousedown={close} oncontextmenu={(e) => { e.preventDefault(); close(); }}></div>
  <div class="menu" tabindex="-1" bind:this={menuEl} style="left: {pos.x}px; top: {pos.y}px; visibility: {placed ? 'visible' : 'hidden'}" role="menu" onkeydown={onMenuKey}>
    {#each app.menu.items as item}
      {#if item.rating}
        <div class="rate" role="group" aria-label="Rating">
          <Rating value={item.rating.value} size={18} onrate={(n) => { close(); item.rating?.set(n); }} />
        </div>
      {:else}
      <button role="menuitem" onclick={() => { close(); item.action(); }}>
        {#if item.icon}<Icon name={item.icon} size={15} />{/if}
        {item.label}
      </button>
      {/if}
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
    max-height: calc(100vh - 16px);
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 5px;
    border-radius: var(--r-md);
    background: rgba(32, 34, 54, 0.96);
    backdrop-filter: blur(24px) saturate(1.5);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 16px 40px rgba(0, 0, 0, 0.5);
    animation: pop 0.12s ease-out;
  }
  .rate {
    display: flex;
    align-items: center;
    height: 32px;
    padding: 0 8px;
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

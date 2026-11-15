<script lang="ts" module>
  /** The dialogs open now, the newest (top-most) last: Escape closes only that one. */
  const open: { id: symbol; close: () => void }[] = [];

  /** Closes the top-most open dialog (Android Back). False when none is open. */
  export function closeTopModal(): boolean {
    const top = open[open.length - 1];
    if (!top) return false;
    top.close();
    return true;
  }
</script>

<script lang="ts">
  import { onMount, type Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  let { title, onclose, children, footer, width = "520px" }: { title: string; onclose: () => void; children: Snippet; footer?: Snippet; width?: string } =
    $props();

  const me = Symbol("dialog");
  /** What had the keyboard when the dialog opened (a song row, a button): it gets it back. */
  const opener = typeof document === "undefined" ? null : (document.activeElement as HTMLElement | null);
  onMount(() => {
    open.push({ id: me, close: () => onclose() });
    return () => {
      const i = open.findIndex((m) => m.id === me);
      if (i >= 0) open.splice(i, 1);
      try {
        giveFocusBack();
      } catch {
        // Focus is a nicety: closing never fails over it.
      }
    };
  });

  /** Closing: the keyboard goes back where it was (as the context menu does), unless what closed
   *  the dialog moved it on purpose (another dialog's field, a page's box). */
  function giveFocusBack() {
    const el = opener;
    if (!el || el === document.body || !el.isConnected || typeof el.focus !== "function") return;
    const now = document.activeElement;
    if (now && now !== document.body && now !== el && !panel?.contains(now)) return;
    if ((el as HTMLButtonElement).disabled || el.closest("[inert], [hidden], [aria-hidden='true']")) return;
    el.focus({ preventScroll: true });
  }

  /** Escape closes the dialog, unless something inside it (an open dropdown, a menu) already
   *  used the key, or another dialog is open on top of it. */
  function onKey(e: KeyboardEvent) {
    if (e.key !== "Escape" || e.defaultPrevented || open[open.length - 1]?.id !== me) return;
    e.preventDefault();
    onclose();
  }

  let panel = $state<HTMLDivElement>();
  $effect(() => {
    // Focus the first field so the dialog can be used straight from the keyboard.
    panel?.querySelector<HTMLElement>("input, button:not(.x), [tabindex]")?.focus();
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="overlay" role="presentation" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div class="panel" bind:this={panel} role="dialog" aria-modal="true" aria-label={title} style="width: {width}">
    <header>
      <h2>{title}</h2>
      <button class="icon-btn x" onclick={onclose} aria-label="Close"><Icon name="close" size={16} /></button>
    </header>
    <div class="body">{@render children()}</div>
    {#if footer}<footer>{@render footer()}</footer>{/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 80;
    display: grid;
    place-items: center;
    padding: 40px;
    background: color-mix(in srgb, var(--ink) 60%, transparent);
    backdrop-filter: blur(6px);
    animation: fade 0.15s ease-out;
  }
  /* Never taller than what can be seen (dvh: the visible viewport, without a phone's browser bars
     or keyboard; vh where it's unknown): the title and the buttons stay on screen and the body
     scrolls between them. */
  .panel {
    max-width: calc(100vw - 24px);
    max-height: calc(100vh - 80px);
    max-height: calc(100dvh - 80px);
    min-height: 0;
    display: flex;
    flex-direction: column;
    border-radius: var(--r-lg);
    background: rgba(30, 32, 50, 0.98);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 30px 80px rgba(0, 0, 0, 0.55);
    animation: pop 0.18s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 18px 16px 6px 22px;
  }
  h2 {
    margin: 0;
    font-family: var(--display);
    font-size: 20px;
    letter-spacing: -0.015em;
  }
  /* Scrolls when the dialog is taller than the screen. Dropdowns (Select) inside a dialog open as
     a layer of their own (position: fixed), so the scrolling doesn't cut them off. */
  .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 8px 22px 18px;
  }
  footer {
    flex: none;
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    padding: 14px 22px 18px;
    border-top: 1px solid var(--line);
  }
  /* A short window (a phone on its side, ~360 px): less space around the dialog and in its
     title and buttons, so more of the body shows. */
  @media (max-height: 560px) {
    .overlay {
      padding: 8px;
    }
    .panel {
      max-height: calc(100vh - 16px);
      max-height: calc(100dvh - 16px);
    }
    header {
      padding: 10px 12px 2px 18px;
    }
    footer {
      padding: 8px 18px 10px;
    }
    .body {
      padding: 6px 18px 10px;
    }
  }
  /* Phones: a sheet that rises from the bottom, the full width of the screen. */
  @media (max-width: 720px) {
    .overlay {
      align-items: end;
      padding: 0 8px calc(8px + env(safe-area-inset-bottom));
    }
    .panel {
      width: 100% !important;
      max-height: 85vh;
      max-height: calc(100dvh - 16px - env(safe-area-inset-top) - env(safe-area-inset-bottom));
      border-radius: 24px;
      animation: rise 0.28s cubic-bezier(0.2, 0.8, 0.2, 1);
    }
    footer {
      flex-wrap: wrap;
    }
  }
  /* The Android app at any width (a phone on its side, a tablet): the same sheet, inside the
     screen's safe area. */
  :global(html.phone) .overlay {
    align-items: end;
    padding: calc(8px + env(safe-area-inset-top)) calc(8px + env(safe-area-inset-right)) calc(8px + env(safe-area-inset-bottom)) calc(8px + env(safe-area-inset-left));
  }
  :global(html.phone) .panel {
    max-height: calc(100vh - 16px);
    max-height: calc(100dvh - 16px - env(safe-area-inset-top) - env(safe-area-inset-bottom));
    max-width: 100%;
    border-radius: 24px;
    animation: rise 0.28s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  :global(html.phone) footer {
    flex-wrap: wrap;
  }
  @media (max-width: 720px) {
    :global(html.phone) .panel {
      width: 100% !important;
    }
  }
  @keyframes rise {
    from {
      transform: translateY(40px);
      opacity: 0;
    }
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.98);
    }
  }
</style>

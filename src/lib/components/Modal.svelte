<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";

  let { title, onclose, children, footer, width = "520px" }: { title: string; onclose: () => void; children: Snippet; footer?: Snippet; width?: string } =
    $props();

  let panel = $state<HTMLDivElement>();
  $effect(() => {
    // Focus the first field so the dialog can be used straight from the keyboard.
    panel?.querySelector<HTMLElement>("input, button:not(.x), [tabindex]")?.focus();
  });
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

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
  .panel {
    max-width: calc(100vw - 24px);
    max-height: calc(100vh - 80px);
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
  /* Not scrollable on purpose: dropdowns inside must be able to overflow the dialog. */
  .body {
    padding: 8px 22px 18px;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    padding: 14px 22px 18px;
    border-top: 1px solid var(--line);
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
      border-radius: 24px;
      animation: rise 0.28s cubic-bezier(0.2, 0.8, 0.2, 1);
    }
    footer {
      flex-wrap: wrap;
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

<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Icon from "./Icon.svelte";

  const win = getCurrentWindow();
</script>

<!-- Frameless window: this strip drags the window; the buttons sit on the right like native ones. -->
<div class="titlebar" data-tauri-drag-region>
  <div class="controls">
    <button aria-label="Minimize" onclick={() => win.minimize()}><Icon name="minimize" size={16} /></button>
    <button aria-label="Maximize" onclick={() => win.toggleMaximize()}><Icon name="maximize" size={14} /></button>
    <button class="close" aria-label="Close" onclick={() => win.close()}><Icon name="close" size={16} /></button>
  </div>
</div>

<style>
  /* Same height as the page's top bar (52 px), so the window buttons line up with Back. */
  .titlebar {
    position: fixed;
    inset: 0 0 auto 0;
    height: 52px;
    z-index: 50;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding-right: 8px;
    pointer-events: none;
  }
  .controls {
    display: flex;
    gap: 2px;
    pointer-events: auto;
  }
  .controls button {
    width: 42px;
    height: 34px;
    border-radius: var(--r-sm);
    display: grid;
    place-items: center;
    color: var(--text-2);
  }
  .controls button:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }
  .controls .close:hover {
    background: #c42b1c;
    color: white;
  }
</style>

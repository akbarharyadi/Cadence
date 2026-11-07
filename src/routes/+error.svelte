<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { recentCrash, reportCrash } from "#lib/diag.js";

  // Last resort: an error the per-view guards in +page.svelte didn't catch. On Android this showed
  // up as a bare "500 Internal Error" after the screen was turned off and on again (0.2.32). The
  // music keeps playing in Rust, so the page can simply load again: do that once by itself, and
  // only if it fails again right away, show what the error was. Either way the error and the steps
  // before it are kept, and can be sent as a GitHub issue (here, or from the toast after the reload).
  const KEY = "cadence-error-reload";
  let reloading = $state(true);

  onMount(() => {
    const last = Number(sessionStorage.getItem(KEY) ?? 0);
    if (Date.now() - last > 30_000) {
      sessionStorage.setItem(KEY, String(Date.now()));
      location.reload();
      return;
    }
    reloading = false;
  });
</script>

{#if !reloading}
  <div class="err" role="alert">
    <h1>Cadence ran into a problem</h1>
    <p class="msg">{page.error?.message ?? "Unknown error"}</p>
    <div class="actions">
      <button onclick={() => location.reload()}>Reload</button>
      {#if recentCrash()}<button class="ghost" onclick={() => { const c = recentCrash(); if (c) reportCrash(c); }}>Report on GitHub</button>{/if}
    </div>
  </div>
{/if}

<style>
  .err {
    min-height: 100vh;
    display: grid;
    place-content: center;
    gap: 12px;
    padding: 24px;
    text-align: center;
    color: #fff;
    background: #0f0f16;
  }
  h1 {
    margin: 0;
    font-size: 20px;
  }
  .msg {
    margin: 0;
    opacity: 0.7;
    font-size: 13px;
    max-width: 480px;
    word-break: break-word;
    user-select: text;
  }
  .actions {
    display: flex;
    gap: 10px;
    justify-content: center;
  }
  button.ghost {
    background: rgba(255, 255, 255, 0.12);
    color: inherit;
  }
  button {
    padding: 8px 20px;
    border-radius: 999px;
    border: none;
    background: rgba(255, 255, 255, 0.92);
    color: #111;
    font: inherit;
    font-weight: 600;
  }
</style>

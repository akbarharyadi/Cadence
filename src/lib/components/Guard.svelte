<script lang="ts">
  import type { Snippet } from "svelte";
  import { errorMessage, recordCrash, reportCrash, type Crash } from "#lib/diag.js";

  /**
   * Keeps an error in one part of the window inside that part. Without it, SvelteKit's own
   * boundary catches any error thrown while a component renders or runs an effect and swaps the
   * WHOLE app for a bare "500 Internal Error" page (seen on Android, 0.2.32). Here the rest of the
   * window keeps working, and the message says what broke.
   */
  let {
    children,
    label = "This part of Cadence",
    overlay = false,
    onclose,
  }: { children: Snippet; label?: string; overlay?: boolean; onclose?: () => void } = $props();

  let crash = $state<Crash | null>(null);
  function onFailed(e: unknown) {
    console.error(`[Cadence] ${label} failed:`, e);
    crash = recordCrash(e, label);
  }
</script>

<svelte:boundary onerror={onFailed}>
  {@render children()}
  {#snippet failed(error, reset)}
    <div class="guard" class:overlay role="alert">
      <div class="card">
        <h2>{label} ran into a problem</h2>
        <p class="msg">{errorMessage(error)}</p>
        <div class="actions">
          <button class="btn" onclick={reset}>Try again</button>
          {#if crash}<button class="btn ghost" onclick={() => crash && reportCrash(crash)}>Report on GitHub</button>{/if}
          {#if onclose}<button class="btn ghost" onclick={onclose}>Close</button>{/if}
        </div>
      </div>
    </div>
  {/snippet}
</svelte:boundary>

<style>
  .guard {
    display: grid;
    place-items: center;
    padding: 48px 20px;
  }
  .guard.overlay {
    position: fixed;
    inset: 0;
    z-index: 900;
    background: rgba(10, 10, 16, 0.92);
  }
  .card {
    max-width: 440px;
    text-align: center;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 18px;
  }
  .msg {
    margin: 0 0 18px;
    opacity: 0.7;
    font-size: 13px;
    word-break: break-word;
    user-select: text;
  }
  .actions {
    display: flex;
    gap: 10px;
    justify-content: center;
  }
  .btn {
    padding: 8px 18px;
    border-radius: 999px;
    border: none;
    background: rgba(255, 255, 255, 0.92);
    color: #111;
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }
  .btn.ghost {
    background: rgba(255, 255, 255, 0.12);
    color: inherit;
  }
</style>

<script lang="ts">
  // "A new version is ready" card, above the player.
  import { fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { updates } from "$lib/updates.svelte";
  import Icon from "./Icon.svelte";

  const isAndroid = navigator.userAgent.includes("Android");
  const show = $derived(
    !updates.dismissed && (updates.phase === "ready" || updates.phase === "available" || (isAndroid && updates.phase === "downloading" && updates.next !== "")),
  );
</script>

{#if show}
  <div class="update" role="status" transition:fly={{ y: 16, duration: 280, easing: cubicOut }}>
    <span class="mark"><Icon name="download" size={18} /></span>
    <div class="text">
      <strong>Cadence {updates.next}</strong>
      <span>
        {#if updates.phase === "downloading"}Downloading… {Math.round(updates.progress * 100)}%
        {:else if updates.message}{updates.message}
        {:else if updates.phase === "ready"}Downloaded. Restart Cadence to finish updating.
        {:else}A new version is available.{/if}
      </span>
    </div>
    {#if updates.phase !== "downloading"}
      <button class="pill" onclick={() => updates.apply()}>{updates.phase === "ready" ? "Restart" : "Update"}</button>
      <button class="icon-btn" onclick={() => (updates.dismissed = true)} aria-label="Later" title="Later"><Icon name="close" size={15} /></button>
    {/if}
    {#if updates.phase === "downloading"}<div class="bar" style="transform: scaleX({updates.progress})"></div>{/if}
  </div>
{/if}

<style>
  /* Bottom right, above the player: clear of the sidebar and its Sound / Settings buttons. */
  .update {
    position: fixed;
    right: 24px;
    bottom: 112px;
    z-index: 35;
    display: flex;
    align-items: center;
    gap: 12px;
    max-width: min(460px, calc(100vw - 48px));
    padding: 12px 12px 12px 14px;
    border-radius: 16px;
    overflow: hidden;
    background: color-mix(in srgb, var(--tint) 30%, var(--panel));
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 16px 40px rgba(0, 0, 0, 0.5);
  }
  .mark {
    width: 36px;
    height: 36px;
    flex: none;
    display: grid;
    place-items: center;
    border-radius: 10px;
    background: linear-gradient(135deg, #5d4dff, #ff6fb1);
    color: white;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    font-size: 13px;
    color: var(--text-2);
  }
  .text strong {
    color: var(--text);
    font-size: 14px;
  }
  .bar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: var(--accent);
    transform-origin: left;
    transition: transform 0.2s;
  }
  @media (max-width: 720px) {
    .update {
      left: 8px;
      right: 8px;
      max-width: none;
      bottom: calc(140px + env(safe-area-inset-bottom));
    }
  }
</style>

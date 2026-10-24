<script lang="ts">
  // Android: Cadence isn't allowed to read the music on the phone (e.g. after reinstalling).
  // The library stays as it was; this asks for access again.
  import { fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { app, android } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
</script>

{#if !app.musicAccess && app.tracks.length > 0}
  <div class="access" role="alert" transition:fly={{ y: 16, duration: 280, easing: cubicOut }}>
    <span class="mark"><Icon name="folder" size={18} /></span>
    <div class="text">
      <strong>Cadence can’t read your music</strong>
      <span>Allow access to play the songs on this phone.</span>
    </div>
    <button class="pill" onclick={() => android()?.requestMusicPermission()}>Allow access</button>
  </div>
{/if}

<style>
  .access {
    position: fixed;
    left: 8px;
    right: 8px;
    bottom: calc(140px + env(safe-area-inset-bottom));
    z-index: 35;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 12px 12px 14px;
    border-radius: 16px;
    background: color-mix(in srgb, var(--tint) 30%, #1a1b2b);
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
    background: color-mix(in srgb, var(--warn) 30%, transparent);
    color: var(--warn);
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
</style>

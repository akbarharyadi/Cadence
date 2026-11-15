<script lang="ts">
  // Phone navigation: the sidebar's main places as a tab bar at the bottom of the screen. Library
  // opens a page listing everything else the sidebar has (Artists, playlists, Needs Attention…).
  import { app, type View } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";

  const tabs: { label: string; icon: string; view: View; sections: string[] }[] = [
    { label: "Listen Now", icon: "home", view: { name: "home" }, sections: ["home", "discover", "stats"] },
    { label: "Albums", icon: "album", view: { name: "albums" }, sections: ["albums", "recent"] },
    { label: "Library", icon: "song", view: { name: "library" }, sections: ["library", "songs", "artists", "favorites", "attention", "health", "playlist"] },
    { label: "Search", icon: "search", view: { name: "search", query: "" }, sections: ["search"] },
    { label: "Settings", icon: "settings", view: { name: "settings" }, sections: ["settings", "sound"] },
  ];

  /** The section the page on screen belongs to (playlists are all under Library). */
  const section = $derived(app.section.startsWith("playlist:") ? "playlist" : app.section);
</script>

<nav class="tabs" aria-label="Main">
  {#each tabs as t (t.label)}
    {@const active = t.sections.includes(section)}
    <button class:active onclick={() => app.go(t.view)} aria-current={active ? "page" : undefined}>
      <span class="icon">
        <Icon name={t.icon} size={21} />
        {#if t.view.name === "library" && app.attentionCount > 0}<span class="dot" aria-label="{app.attentionCount} to fix"></span>{/if}
      </span>
      <span>{t.label}</span>
    </button>
  {/each}
</nav>

<style>
  .tabs {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 31;
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    padding: 6px 4px calc(6px + env(safe-area-inset-bottom));
    background: color-mix(in srgb, var(--ink) 88%, transparent);
    backdrop-filter: blur(20px) saturate(1.4);
    box-shadow: 0 -1px 0 var(--line);
  }
  button {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    padding: 6px 0 4px;
    font-size: 10.5px;
    font-weight: 600;
    color: var(--text-3);
  }
  button.active {
    color: var(--accent);
  }
  .icon {
    position: relative;
  }
  /* Needs Attention has something to fix (the number is on the Library page). */
  .dot {
    position: absolute;
    top: -1px;
    right: -4px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
</style>

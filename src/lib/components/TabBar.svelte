<script lang="ts">
  // Phone navigation: the sidebar's main places as a tab bar at the bottom of the screen.
  import { app, type View } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";

  const tabs: { label: string; icon: string; view: View; match: string[] }[] = [
    { label: "Listen Now", icon: "home", view: { name: "home" }, match: ["home"] },
    { label: "Albums", icon: "album", view: { name: "albums" }, match: ["albums", "album", "recent"] },
    { label: "Songs", icon: "song", view: { name: "songs" }, match: ["songs", "artists", "artist", "playlist", "similar"] },
    { label: "Search", icon: "search", view: { name: "search", query: "" }, match: ["search"] },
    { label: "Settings", icon: "settings", view: { name: "settings" }, match: ["settings", "sound"] },
  ];
</script>

<nav class="tabs" aria-label="Main">
  {#each tabs as t (t.label)}
    {@const active = t.match.includes(app.view.name)}
    <button class:active onclick={() => app.go(t.view)} aria-current={active ? "page" : undefined}>
      <Icon name={t.icon} size={21} />
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
</style>

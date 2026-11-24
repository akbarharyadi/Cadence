<script lang="ts">
  // Phones: the tab bar's Library. Everything the desktop sidebar lists, as one page: the parts of
  // the library, the playlists (and a new one), and Sound and Settings.
  import { newSmart } from "#lib/smart.js";
  import Icon from "#lib/components/Icon.svelte";
  import { openPalette } from "#lib/palette.svelte.js";
  import { startExportAll, startImport } from "#lib/playlistTransfer.svelte.js";
  import { app, type View } from "#lib/store.svelte.js";

  const places: { label: string; icon: string; view: View }[] = [
    { label: "Songs", icon: "song", view: { name: "songs" } },
    { label: "Albums", icon: "album", view: { name: "albums" } },
    { label: "Artists", icon: "artist", view: { name: "artists" } },
    { label: "Recently Added", icon: "clock", view: { name: "recent" } },
    { label: "Favorites", icon: "heart", view: { name: "favorites" } },
    { label: "Needs Attention", icon: "edit", view: { name: "attention" } },
    { label: "Library Health", icon: "shield", view: { name: "health" } },
  ];

  function newPlaylistMenu(e: MouseEvent) {
    app.openMenu(e, [
      { label: "New playlist", icon: "playlist", action: () => app.newPlaylist() },
      {
        label: "New smart playlist…",
        icon: "smart",
        action: () => (app.smartEditing = { id: "", name: "", trackIds: [], smart: newSmart(), created: 0, updated: 0 }),
      },
      { label: "Import playlist…", icon: "file", action: () => void startImport() },
      ...(app.playlists.length ? [{ label: "Export all playlists…", icon: "download", action: () => startExportAll() }] : []),
    ]);
  }
</script>

{#snippet row(label: string, icon: string, view: View, badge = 0)}
  <li>
    <button class="item" onclick={() => app.go(view)}>
      <Icon name={icon} size={20} />
      <span class="label">{label}</span>
      {#if badge > 0}<span class="badge" aria-label="{badge} to fix">{badge > 999 ? "999+" : badge}</span>{/if}
      <span class="chev"><Icon name="chevron" size={16} /></span>
    </button>
  </li>
{/snippet}

<div class="page library">
  <h1 class="page-title">Library</h1>

  <!-- Playlists first: the phone has no other place that lists them. -->
  <div class="group first">
    <h2 class="section-title">Playlists</h2>
    <button class="icon-btn add" onclick={newPlaylistMenu} aria-label="New playlist" title="New playlist"><Icon name="plus" size={18} /></button>
  </div>
  <ul class="list">
    {#each app.playlists as p (p.id)}
      {@render row(p.name, p.smart ? "smart" : "playlist", { name: "playlist", id: p.id })}
    {:else}
      <li>
        <button class="item muted" onclick={newPlaylistMenu}><Icon name="plus" size={20} /><span class="label">New playlist</span></button>
      </li>
    {/each}
  </ul>

  <ul class="list more">
    {#each places as p (p.label)}
      {@render row(p.label, p.icon, p.view, p.view.name === "attention" ? app.attentionCount : 0)}
    {/each}
  </ul>

  <ul class="list more">
    <li>
      <button class="item" onclick={openPalette}>
        <Icon name="search" size={20} />
        <span class="label">Find anything</span>
        <span class="chev"><Icon name="chevron" size={16} /></span>
      </button>
    </li>
    {@render row("Sound", "eq", { name: "sound" })}
    {@render row("Settings", "settings", { name: "settings" })}
  </ul>
</div>

<style>
  .page-title {
    margin-bottom: 18px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 4px;
    border-radius: var(--r-lg);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .more {
    margin-top: 26px;
  }
  .group {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 28px 0 10px;
  }
  .group.first {
    margin-top: 0;
  }
  .add {
    background: var(--hover);
  }
  .item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 14px;
    height: 48px;
    padding: 0 12px;
    border-radius: var(--r-md);
    text-align: left;
    color: var(--text);
    font-weight: 500;
  }
  .item:active,
  .item:hover {
    background: var(--active);
  }
  .item :global(svg) {
    color: var(--accent);
  }
  .item.muted {
    color: var(--text-2);
  }
  .label {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .badge {
    min-width: 22px;
    height: 22px;
    padding: 0 7px;
    border-radius: 99px;
    display: grid;
    place-items: center;
    font-size: 12px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
    background: color-mix(in srgb, var(--accent) 28%, transparent);
  }
  .chev :global(svg) {
    color: var(--text-3);
  }
</style>

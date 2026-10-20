<script lang="ts">
  import { newSmart } from "$lib/smart";
  import { app, type View } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";

  let searchInput: HTMLInputElement;

  const library: { label: string; icon: string; view: View }[] = [
    { label: "Recently Added", icon: "clock", view: { name: "recent" } },
    { label: "Albums", icon: "album", view: { name: "albums" } },
    { label: "Artists", icon: "artist", view: { name: "artists" } },
    { label: "Songs", icon: "song", view: { name: "songs" } },
  ];

  const isActive = (v: View) =>
    app.view.name === v.name ||
    (v.name === "albums" && app.view.name === "album") ||
    (v.name === "artists" && app.view.name === "artist");

  function onSearch(e: Event) {
    const q = (e.target as HTMLInputElement).value;
    app.search = q;
    if (q.trim().length > 0) {
      if (app.view.name === "search") app.view = { name: "search", query: q };
      else app.go({ name: "search", query: q });
    } else if (app.view.name === "search") app.back();
  }

  async function newPlaylist() {
    const p = await app.createPlaylist(`Playlist ${app.playlists.filter((x) => !x.smart).length + 1}`);
    app.go({ name: "playlist", id: p.id });
  }

  function newPlaylistMenu(e: MouseEvent) {
    app.openMenu(e, [
      { label: "New playlist", icon: "playlist", action: newPlaylist },
      {
        label: "New smart playlist…",
        icon: "smart",
        action: () => (app.smartEditing = { id: "", name: "", trackIds: [], smart: newSmart(), created: 0, updated: 0 }),
      },
    ]);
  }

  export function focusSearch() {
    searchInput?.focus();
    searchInput?.select();
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if ((e.ctrlKey || e.metaKey) && (e.key === "f" || e.key === "k")) {
      e.preventDefault();
      focusSearch();
    }
  }}
/>

<aside class="sidebar">
  <div class="brand" data-tauri-drag-region>
    <svg class="mark" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 15c2.5-7 4.5-7 6 0s3.5 7 6 0 3.5-7 6 0" /></svg>
    <span>Cadence</span>
  </div>

  <label class="search">
    <Icon name="search" size={15} />
    <input bind:this={searchInput} placeholder="Search your music" value={app.search} oninput={onSearch} spellcheck="false" />
    <kbd>Ctrl K</kbd>
  </label>

  <nav>
    <button class="item" class:active={app.view.name === "home"} onclick={() => app.go({ name: "home" })}>
      <Icon name="home" size={18} />Listen Now
    </button>

    <div class="group">Library</div>
    {#each library as item}
      <button class="item" class:active={isActive(item.view)} onclick={() => app.go(item.view)}>
        <Icon name={item.icon} size={18} />{item.label}
      </button>
    {/each}

    <div class="group with-action">
      Playlists
      <button class="add" onclick={newPlaylistMenu} aria-label="New playlist" title="New playlist"><Icon name="plus" size={15} /></button>
    </div>
    {#each app.playlists as p (p.id)}
      <button class="item" class:active={app.view.name === "playlist" && app.view.id === p.id} onclick={() => app.go({ name: "playlist", id: p.id })}>
        <Icon name={p.smart ? "smart" : "playlist"} size={18} /><span class="label">{p.name}</span>
      </button>
    {:else}
      <button class="item muted" onclick={newPlaylistMenu}><Icon name="plus" size={18} />New playlist</button>
    {/each}
  </nav>

  <div class="bottom">
    {#if app.scan || app.analysis}
      {@const job = app.scan ?? app.analysis!}
      <div class="job">
        <div class="bar"><div style="width: {job.total ? (job.done / job.total) * 100 : 6}%"></div></div>
        <span>{app.scan ? "Importing" : "Measuring loudness"}{job.total ? ` ${job.done} of ${job.total}` : "…"}</span>
      </div>
    {/if}
    <button class="item" class:active={app.view.name === "sound"} onclick={() => app.go({ name: "sound" })}>
      <Icon name="eq" size={18} />Sound
      {#if app.dsp?.eq.enabled || app.dsp?.crossfeed.enabled}<span class="dot" title="Equalizer or crossfeed is on"></span>{/if}
    </button>
    <button class="item" class:active={app.view.name === "settings"} onclick={() => app.go({ name: "settings" })}>
      <Icon name="settings" size={18} />Settings
    </button>
  </div>
</aside>

<style>
  .sidebar {
    width: 240px;
    flex: none;
    display: flex;
    flex-direction: column;
    /* Bottom padding clears the floating player dock. */
    padding: 0 12px 106px;
    background: rgba(16, 17, 26, 0.55);
    border-right: 1px solid var(--line);
    position: relative;
    z-index: 1;
  }
  .brand {
    height: 60px;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 10px 0;
    font-family: var(--display);
    font-weight: 700;
    font-size: 19px;
    letter-spacing: -0.02em;
  }
  .brand > * {
    pointer-events: none;
  }
  .mark {
    width: 24px;
    height: 24px;
    fill: none;
    stroke: var(--accent);
    stroke-width: 2.6;
    stroke-linecap: round;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 8px 0 11px;
    margin: 6px 0 18px;
    border-radius: var(--r-sm);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    color: var(--text-3);
  }
  .search:focus-within {
    box-shadow: inset 0 0 0 1px var(--accent);
    color: var(--text-2);
  }
  .search input {
    flex: 1;
    border: 0;
    outline: 0;
    background: none;
    min-width: 0;
    font-size: 13px;
  }
  .search input:focus-visible {
    outline: none;
  }
  .search input::placeholder {
    color: var(--text-3);
  }
  kbd {
    font: inherit;
    font-size: 10.5px;
    color: var(--text-3);
    padding: 1px 5px;
    border-radius: 4px;
    box-shadow: inset 0 0 0 1px var(--line);
  }
  nav {
    flex: 1;
    overflow-y: auto;
  }
  .group {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
    padding: 18px 10px 6px;
  }
  .with-action {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .add {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    display: grid;
    place-items: center;
    color: var(--text-3);
  }
  .add:hover {
    background: var(--active);
    color: var(--text);
  }
  .label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .item.muted {
    color: var(--text-3);
  }
  .item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 12px;
    height: 36px;
    padding: 0 10px;
    border-radius: var(--r-sm);
    text-align: left;
    color: var(--text-2);
    font-weight: 500;
    transition: background 0.12s, color 0.12s;
  }
  .item :global(svg) {
    color: var(--text-3);
    transition: color 0.12s;
  }
  .item:hover {
    background: var(--hover);
    color: var(--text);
  }
  .item.active {
    background: var(--active);
    color: var(--text);
  }
  .item.active :global(svg) {
    color: var(--accent);
  }
  .dot {
    margin-left: auto;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }
  .bottom {
    padding-top: 8px;
  }
  .job {
    padding: 4px 10px 12px;
    font-size: 12px;
    color: var(--text-2);
  }
  .job .bar {
    height: 3px;
    border-radius: 3px;
    background: var(--line-strong);
    overflow: hidden;
    margin-bottom: 7px;
  }
  .job .bar div {
    height: 100%;
    background: var(--accent);
    transition: width 0.3s;
  }
</style>

<script lang="ts">
  // Choosing a music folder inside Cadence (phones have no usable system folder picker).
  import { api, type DirEntry } from "$lib/api";
  import { android, app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  /** null = the starting places (phone storage, SD cards). */
  let path = $state<string | null>(null);
  let trail = $state<{ name: string; path: string | null }[]>([{ name: "Storage", path: null }]);
  let dirs = $state<DirEntry[]>([]);
  let error = $state("");
  let loading = $state(true);

  async function load(p: string | null) {
    loading = true;
    error = "";
    try {
      // Android itself lists the SD cards and USB drives that are plugged in.
      const volumes = p == null ? JSON.parse(android()?.storageVolumes?.() || "[]") : undefined;
      dirs = await api.listDirs(p, volumes);
    } catch (e) {
      dirs = [];
      error = String(e);
    }
    loading = false;
  }
  $effect(() => {
    load(path);
  });

  function open(d: DirEntry) {
    trail = [...trail, { name: d.name, path: d.path }];
    path = d.path;
  }
  function jump(i: number) {
    trail = trail.slice(0, i + 1);
    path = trail[i].path;
  }
  function close(chosen: string | null) {
    app.folderBrowser?.(chosen);
    app.folderBrowser = null;
  }
  const already = $derived(path != null && app.folders.some((f) => path === f || path!.startsWith(f + "/")));
</script>

<Modal title="Add a music folder" onclose={() => close(null)} width="560px">
  <nav class="trail" aria-label="Folder path">
    {#each trail as t, i (i)}
      {#if i > 0}<Icon name="chevron" size={12} />{/if}
      <button class:here={i === trail.length - 1} onclick={() => jump(i)}>{t.name}</button>
    {/each}
  </nav>

  <ul class="list">
    {#if loading}
      <li class="note">Loading…</li>
    {:else if error}
      <li class="note">{error}</li>
    {:else if dirs.length === 0}
      <li class="note">No folders here.</li>
    {:else}
      {#each dirs as d (d.path)}
        <li>
          <button class="dir" onclick={() => open(d)}>
            <span class="ico"><Icon name="folder" size={18} /></span>
            <span class="text">
              <span class="name">{d.name}</span>
              {#if d.songs}<span class="songs">{d.songs} {d.songs === 1 ? "song" : "songs"}</span>{/if}
            </span>
            {#if d.hasFolders}<Icon name="chevron" size={14} />{/if}
          </button>
        </li>
      {/each}
    {/if}
  </ul>

  {#snippet footer()}
    <button class="pill ghost" onclick={() => close(null)}>Cancel</button>
    <button class="pill" disabled={path == null || already} onclick={() => close(path)}>
      {already ? "Already in your library" : path == null ? "Open a folder first" : `Add “${trail[trail.length - 1].name}”`}
    </button>
  {/snippet}
</Modal>

<style>
  .trail {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-bottom: 10px;
    color: var(--text-3);
  }
  .trail button {
    padding: 4px 8px;
    border-radius: 8px;
    font-size: 13px;
    color: var(--text-2);
  }
  .trail button:hover {
    background: var(--hover);
  }
  .trail .here {
    color: var(--text);
    font-weight: 600;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: min(52vh, 420px);
    overflow-y: auto;
  }
  .note {
    padding: 18px 8px;
    color: var(--text-3);
  }
  .dir {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 8px;
    border-radius: 10px;
    text-align: left;
    color: var(--text-3);
  }
  .dir:hover {
    background: var(--hover);
  }
  .ico {
    width: 36px;
    height: 36px;
    flex: none;
    display: grid;
    place-items: center;
    border-radius: 9px;
    background: var(--surface-2);
    color: var(--accent);
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .name {
    color: var(--text);
    font-weight: 550;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .songs {
    font-size: 12px;
    color: var(--gold);
  }
</style>

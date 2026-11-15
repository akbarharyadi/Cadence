<script lang="ts">
  // The dialogs of playlist import and export, mounted once in the page; `playlistTransfer.svelte.ts`
  // says which is open.
  import { importPaths, transfer } from "#lib/playlistTransfer.svelte.js";
  import Modal from "./Modal.svelte";
  import PlaylistExportDialog from "./PlaylistExportDialog.svelte";
  import PlaylistImportDialog from "./PlaylistImportDialog.svelte";

  const size = (b: number) => (b >= 1e6 ? `${(b / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(b / 1e3))} KB`);

  function pick(path: string) {
    transfer.choosing = null;
    void importPaths([path]);
  }
</script>

{#if transfer.exporting}<PlaylistExportDialog target={transfer.exporting} />{/if}

{#if transfer.choosing}
  {@const found = transfer.choosing}
  <Modal title="Import playlist" onclose={() => (transfer.choosing = null)} width="460px">
    {#if found.files.length}
      <p class="hint">Playlist files in {found.dir}:</p>
      <ul class="files">
        {#each found.files as f (f.path)}
          <li>
            <span class="name">{f.name}</span>
            <span class="secondary">{size(f.size)}</span>
            <button class="link" onclick={() => pick(f.path)} aria-label="Import {f.name}">Import</button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="hint">No playlist files found. Put .m3u, .m3u8, .csv or .xspf files in {found.dir || "Documents › Cadence"} on this phone, then try again.</p>
    {/if}
    {#snippet footer()}
      <button class="pill ghost" onclick={() => (transfer.choosing = null)}>Close</button>
    {/snippet}
  </Modal>
{/if}

{#if transfer.reading}
  <Modal title="Reading playlist" onclose={() => {}} width="360px">
    <p class="hint">Reading the file and matching its songs to your library…</p>
  </Modal>
{/if}

{#if transfer.imports[0]}
  {#key transfer.imports[0].source}
    <PlaylistImportDialog item={transfer.imports[0]} remaining={transfer.imports.length - 1} />
  {/key}
{/if}

<style>
  .hint {
    margin: 0;
    color: var(--text-2);
    font-size: 13.5px;
    overflow-wrap: anywhere;
  }
  .files {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
  }
  .files li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 12px;
    padding: 7px 6px;
    border-radius: var(--r-sm);
  }
  .files li:hover {
    background: var(--hover);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>

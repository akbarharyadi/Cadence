<script lang="ts">
  import { api } from "$lib/api";
  import Icon from "$lib/components/Icon.svelte";
  import { app } from "$lib/store.svelte";

  /** Folders were added but no music was found in them. */
  const nothingFound = $derived(app.folders.length > 0 && !app.scan);

  async function addFolder() {
    await app.pickFolders();
  }
</script>

<div class="welcome">
  <div class="drop">
    <div class="logo"><Icon name="note" size={40} /></div>
    {#if nothingFound}
      <h1>No music found yet</h1>
      <p>
        Cadence found no songs in {app.folders.length === 1 ? "this folder" : "these folders"} or the folders inside them. It plays FLAC,
        ALAC, WAV, AIFF, MP3, AAC and Ogg files. Copy some in, or choose another folder.
      </p>
      <ul class="folders">
        {#each app.folders as f (f)}<li title={f}><Icon name="folder" size={16} /><span>{f}</span></li>{/each}
      </ul>
      <div class="actions">
        <button class="pill" onclick={addFolder}><Icon name="folder" size={16} />Choose another folder</button>
        <button class="pill ghost" onclick={() => api.rescan()}><Icon name="refresh" size={16} />Look again</button>
      </div>
    {:else}
      <h1>Bring your music</h1>
      <p>Drop a folder of FLAC, ALAC or other music anywhere on this window, or choose one. Cadence plays it in full lossless quality.</p>
      {#if app.scan}
        <p class="importing">Importing {app.scan.total ? `${app.scan.done} of ${app.scan.total} songs` : "…"}</p>
      {:else}
        <button class="pill" onclick={addFolder}><Icon name="folder" size={16} />Choose a folder</button>
      {/if}
    {/if}
  </div>
</div>

<style>
  .welcome {
    height: 100%;
    display: grid;
    place-items: center;
    padding: 40px;
  }
  .drop {
    max-width: 520px;
    text-align: center;
    padding: 48px 56px;
    border-radius: 22px;
    border: 2px dashed var(--line-strong);
    background: radial-gradient(ellipse at top, color-mix(in srgb, var(--accent) 12%, transparent), transparent 70%);
  }
  .logo {
    width: 76px;
    height: 76px;
    margin: 0 auto 18px;
    border-radius: 20px;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--ink);
    box-shadow: 0 12px 34px color-mix(in srgb, var(--accent) 45%, transparent);
  }
  h1 {
    margin: 0 0 8px;
    font-family: var(--display);
    letter-spacing: -0.02em;
    font-size: 28px;
  }
  p {
    color: var(--text-2);
    margin: 0 0 22px;
    line-height: 1.5;
  }
  .importing {
    color: var(--text);
    margin: 0;
  }
  .pill {
    height: 40px;
    padding: 0 22px;
  }
  .folders {
    list-style: none;
    margin: 0 0 22px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .folders li {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-width: 0;
    color: var(--text-3);
    font-size: 13px;
  }
  .folders span {
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px;
  }
</style>

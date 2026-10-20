<script lang="ts">
  import Icon from "$lib/components/Icon.svelte";
  import { app } from "$lib/store.svelte";

  async function addFolder() {
    await app.pickFolders();
  }
</script>

<div class="welcome">
  <div class="drop">
    <div class="logo"><Icon name="note" size={40} /></div>
    <h1>Bring your music</h1>
    <p>Drop a folder of FLAC, ALAC or other music anywhere on this window, or choose one. Cadence plays it in full lossless quality.</p>
    {#if app.scan}
      <p class="importing">Importing {app.scan.total ? `${app.scan.done} of ${app.scan.total} songs` : "…"}</p>
    {:else}
      <button class="pill" onclick={addFolder}><Icon name="folder" size={16} />Choose a folder</button>
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
</style>

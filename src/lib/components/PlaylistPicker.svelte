<script lang="ts">
  import { app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  const tracks = $derived(app.picker ?? []);
  const lists = $derived(app.playlists.filter((p) => !p.smart));
  let newName = $state("");

  const close = () => (app.picker = null);

  async function create() {
    const name = newName.trim();
    if (!name) return;
    const p = await app.createPlaylist(name, tracks);
    app.showToast(`Created “${p.name}” with ${tracks.length === 1 ? `“${tracks[0].title}”` : `${tracks.length} songs`}`);
    close();
  }
</script>

<Modal title={tracks.length === 1 ? `Add “${tracks[0].title}” to a playlist` : `Add ${tracks.length} songs to a playlist`} onclose={close} width="460px">
  <form class="new" onsubmit={(e) => { e.preventDefault(); create(); }}>
    <input class="field" bind:value={newName} placeholder="New playlist name" aria-label="New playlist name" />
    <button class="pill" type="submit" disabled={!newName.trim()}><Icon name="plus" size={15} />Create</button>
  </form>
  {#if lists.length}
    <ul>
      {#each lists as p (p.id)}
        <li>
          <button onclick={async () => { await app.addToPlaylist(p, tracks); close(); }}>
            <Icon name="playlist" size={18} />
            <span class="name">{p.name}</span>
            <span class="count">{p.trackIds.length} songs</span>
          </button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="empty">You don’t have any playlists yet. Name one above to start it with {tracks.length === 1 ? "this song" : "these songs"}.</p>
  {/if}
</Modal>

<style>
  .new {
    display: flex;
    gap: 10px;
    margin-bottom: 14px;
  }
  .new .field {
    flex: 1;
    font-size: 14px;
    padding: 8px 12px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 50vh;
    overflow-y: auto;
  }
  li button {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    text-align: left;
    color: var(--text-2);
  }
  li button:hover {
    background: var(--active);
    color: var(--text);
  }
  .name {
    flex: 1;
    color: var(--text);
    font-weight: 500;
  }
  .count {
    font-size: 12px;
    color: var(--text-3);
  }
  .empty {
    color: var(--text-2);
    font-size: 13px;
  }
</style>

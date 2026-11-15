<script lang="ts">
  import type { Playlist } from "#lib/api.js";
  import { app } from "#lib/store.svelte.js";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  const tracks = $derived(app.picker ?? []);
  const lists = $derived(app.playlists.filter((p) => !p.smart));
  let newName = $state("");
  let busy = $state(false);
  let error = $state("");

  const close = () => (app.picker = null);

  async function create() {
    const name = newName.trim();
    if (!name) return;
    if (busy) return;
    busy = true;
    try {
      const p = await app.createPlaylist(name, tracks);
      app.showToast(`Created “${p.name}” with ${tracks.length === 1 ? `“${tracks[0].title}”` : `${tracks.length} songs`}`);
      close();
    } catch (e) {
      error = `Couldn’t create the playlist: ${e}`;
    } finally {
      busy = false;
    }
  }

  /** Songs the playlist already has: ask before adding them again (another version of a song is a
   *  fine reason to). Nothing is remembered: the question comes every time. */
  let confirm = $state<{ p: Playlist; dup: number; total: number } | null>(null);
  function ask(p: Playlist) {
    if (busy) return;
    const ids = new Set(tracks.map((t) => t.id));
    const have = new Set(p.trackIds);
    const dup = [...ids].filter((id) => have.has(id)).length;
    if (dup === 0) add(p, "skip");
    else confirm = { p, dup, total: ids.size };
  }

  async function add(p: Playlist, dupes: "skip" | "add") {
    if (busy) return;
    confirm = null;
    busy = true;
    try {
      await app.addToPlaylist(p, tracks, dupes);
      close();
    } catch (e) {
      error = `Couldn’t add to “${p.name}”: ${e}`;
    } finally {
      busy = false;
    }
  }

  /** Songs the library still has (ids of files that went away stay in the playlist but aren't songs). */
  const countOf = (p: Playlist) => p.trackIds.filter((id) => app.trackById.has(id)).length;
</script>

<Modal title={tracks.length === 1 ? `Add “${tracks[0].title}” to a playlist` : `Add ${tracks.length} songs to a playlist`} onclose={close} width="460px">
  <form class="new" onsubmit={(e) => { e.preventDefault(); create(); }}>
    <input class="field" bind:value={newName} placeholder="New playlist name" aria-label="New playlist name" />
    <button class="pill" type="submit" disabled={!newName.trim()}><Icon name="plus" size={15} />Create</button>
  </form>
  {#if error}<p class="error">{error}</p>{/if}
  {#if lists.length}
    <ul>
      {#each lists as p (p.id)}
        <li>
          <button onclick={() => ask(p)} disabled={busy}>
            <Icon name="playlist" size={18} />
            <span class="name">{p.name}</span>
            <span class="count">{countOf(p)} {countOf(p) === 1 ? "song" : "songs"}</span>
          </button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="empty">You don’t have any playlists yet. Name one above to start it with {tracks.length === 1 ? "this song" : "these songs"}.</p>
  {/if}
</Modal>

{#if confirm}
  {@const c = confirm}
  <Modal title="Already in this playlist" onclose={() => (confirm = null)} width="440px">
    <p class="ask">
      {#if c.total === 1}“{tracks[0]?.title}” is already in “{c.p.name}”.
      {:else if c.dup === c.total}All {c.total} songs are already in “{c.p.name}”.
      {:else}{c.dup} of {c.total} songs are already in this playlist.{/if}
    </p>
    {#snippet footer()}
      <button class="pill ghost" onclick={() => (confirm = null)}>Cancel</button>
      {#if c.dup < c.total}<button class="pill ghost" onclick={() => add(c.p, "skip")}>Skip duplicates</button>{/if}
      <button class="pill" onclick={() => add(c.p, "add")}>Add anyway</button>
    {/snippet}
  </Modal>
{/if}

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
  .error {
    margin: 0 0 12px;
    color: var(--warn);
    font-size: 13px;
  }
  .empty {
    color: var(--text-2);
    font-size: 13px;
  }
  .ask {
    margin: 4px 0 6px;
    color: var(--text-2);
  }
</style>

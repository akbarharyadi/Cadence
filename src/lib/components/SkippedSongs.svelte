<script lang="ts">
  // Every skipped song in one place: the ones you chose (with "Stop skipping") and, when that
  // switch is on, the instrumental versions. Searchable, so it stays usable with hundreds.
  import { artUrl, type Track } from "$lib/api";
  import { app, isInstrumental } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import Segmented from "./Segmented.svelte";

  const fold = (s: string) => s.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase();
  const byTitle = (a: Track, b: Track) => a.title.localeCompare(b.title);

  const chosen = $derived(app.skipped.map((id) => app.trackById.get(id)).filter((t): t is Track => !!t).sort(byTitle));
  const instrumental = $derived(
    app.skipInstrumentals ? app.tracks.filter((t) => !app.skippedSet.has(t.id) && isInstrumental(t.title)).sort(byTitle) : [],
  );

  let tab = $state<"chosen" | "instrumental">("chosen");
  let query = $state("");
  const list = $derived(tab === "chosen" ? chosen : instrumental);
  const shown = $derived.by(() => {
    const q = fold(query.trim());
    return q ? list.filter((t) => fold(`${t.title} ${t.artist} ${t.album}`).includes(q)) : list;
  });

  async function unskipAll() {
    await app.setSkipped(chosen, false);
  }

  const close = () => (app.skippedOpen = false);
</script>

<Modal title="Skipped songs" onclose={close} width="620px">
  {#if app.skipInstrumentals}
    <div class="tabs">
      <Segmented
        label="Which skipped songs"
        options={[
          { value: "chosen", label: `You chose (${chosen.length})` },
          { value: "instrumental", label: `Instrumental versions (${instrumental.length})` },
        ]}
        value={tab}
        onchange={(v) => (tab = v)}
      />
    </div>
  {/if}

  {#if list.length > 6}
    <label class="search">
      <Icon name="search" size={16} />
      <input class="field" bind:value={query} placeholder="Find a song, artist or album" aria-label="Find a skipped song" />
    </label>
  {/if}

  {#if tab === "instrumental"}
    <p class="note">Skipped because <b>Skip instrumental versions</b> is on. Turn it off in Settings to play them in lists again.</p>
  {/if}

  <ul class="list" aria-label="Skipped songs">
    {#each shown as t (t.id)}
      {@const album = app.albumById.get(t.albumId)}
      <li>
        <button class="song" ondblclick={() => app.playTracks([t], 0, true)} title="Double-click to play">
          <span class="thumb">{#if album?.artSmall}<img src={artUrl(album.artSmall)} alt="" loading="lazy" />{/if}</span>
          <span class="text">
            <span class="title">{t.title}</span>
            <span class="sub">{t.artist}{t.album ? `, ${t.album}` : ""}</span>
          </span>
        </button>
        {#if tab === "chosen"}
          <button class="pill ghost small" onclick={() => app.setSkipped([t], false)}>Stop skipping</button>
        {/if}
      </li>
    {:else}
      <li class="empty">
        {#if query}No skipped songs match “{query}”.
        {:else if tab === "chosen"}No songs skipped. Choose <b>Skip This Song</b> in a song’s menu, or the ⊘ button in the player.
        {:else}No instrumental versions in your library.{/if}
      </li>
    {/each}
  </ul>

  {#snippet footer()}
    {#if tab === "chosen" && chosen.length > 1}
      <button class="link left" onclick={unskipAll}>Stop skipping all {chosen.length}</button>
    {:else}<span class="left"></span>{/if}
    <button class="pill" onclick={close}>Done</button>
  {/snippet}
</Modal>

<style>
  .tabs {
    margin-bottom: 14px;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .search .field {
    flex: 1;
    font-size: 14px;
    padding: 8px 11px;
  }
  .note {
    margin: 0 0 10px;
    font-size: 13px;
    color: var(--text-2);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: min(440px, 52vh);
    overflow-y: auto;
  }
  .list li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 4px 4px 0;
    border-radius: var(--r-sm);
    content-visibility: auto;
    contain-intrinsic-size: auto 52px;
  }
  .list li:hover {
    background: var(--hover);
  }
  .song {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 2px 6px;
    text-align: left;
  }
  .thumb {
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .text {
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .title,
  .sub {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title {
    font-weight: 550;
    font-size: 14px;
  }
  .sub {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .small {
    height: 30px;
    padding: 0 12px;
    font-size: 12.5px;
    flex: none;
  }
  .empty {
    padding: 18px 8px !important;
    color: var(--text-2);
    font-size: 13.5px;
  }
  .left {
    margin-right: auto;
    align-self: center;
    font-size: 13px;
  }
</style>

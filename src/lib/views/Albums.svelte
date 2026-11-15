<script lang="ts">
  import { untrack } from "svelte";
  import AlbumCard from "#lib/components/AlbumCard.svelte";
  import Icon from "#lib/components/Icon.svelte";
  import Select from "#lib/components/Select.svelte";
  import { app } from "#lib/store.svelte.js";
  import type { Album } from "#lib/api.js";

  let { recent = false }: { recent?: boolean } = $props();

  type Sort = "artist" | "title" | "year" | "added";
  let sort = $state<Sort>("artist");
  const sorts: { value: Sort; label: string }[] = [
    { value: "artist", label: "Artist" },
    { value: "title", label: "Title" },
    { value: "year", label: "Year, newest first" },
    { value: "added", label: "Date added" },
  ];

  const cmp: Record<Sort, (a: Album, b: Album) => number> = {
    artist: (a, b) => a.artist.localeCompare(b.artist) || (a.year ?? 0) - (b.year ?? 0),
    title: (a, b) => a.title.localeCompare(b.title),
    year: (a, b) => (b.year ?? 0) - (a.year ?? 0) || a.title.localeCompare(b.title),
    added: (a, b) => b.added - a.added,
  };
  const sorted = $derived([...app.albums].sort(cmp[recent ? "added" : sort]));

  // A thousand album cards take ~100 ms to lay out: the first screens appear at once and the rest
  // are added a few hundred at a time while the browser is idle (no change to how it looks).
  const FIRST = 120;
  const STEP = 240;
  // WebKit (macOS, Linux) has no requestIdleCallback.
  const idle = (f: () => void) =>
    typeof requestIdleCallback === "function" ? requestIdleCallback(f, { timeout: 200 }) : (setTimeout(f, 30) as unknown as number);
  const cancelIdle = (h: number) => (typeof cancelIdleCallback === "function" ? cancelIdleCallback(h) : clearTimeout(h));
  let limit = $state(FIRST);
  // The grid starts over (small, then grows) only when the page or the sort the listener chose
  // changes. A library reload (after a cover or tag edit) only changes how many there are: the cards
  // already shown stay, so the grid doesn't collapse and the scroll position holds.
  let startedFor = "";
  $effect(() => {
    const total = sorted.length;
    const key = `${recent}|${sort}`;
    // (`limit` is read untracked: this effect must follow the library and the sort, not its own growth.)
    if (untrack(() => key !== startedFor)) {
      startedFor = key;
      limit = FIRST;
    }
    let handle = 0;
    const more = () => {
      limit = Math.min(total, limit + STEP);
      if (limit < total) handle = idle(more);
    };
    if (untrack(() => total > limit)) handle = idle(more);
    return () => cancelIdle(handle);
  });
  const shown = $derived(sorted.length > limit ? sorted.slice(0, limit) : sorted);

  /** Recently Added is grouped by when the albums arrived. */
  const groups = $derived.by(() => {
    if (!recent) return [];
    const now = Date.now() / 1000;
    const day = 86400;
    const label = (t: number) => {
      const age = now - t;
      if (age < day) return "Today";
      if (age < 7 * day) return "This week";
      if (age < 31 * day) return "This month";
      return new Date(t * 1000).toLocaleDateString(undefined, { month: "long", year: "numeric" });
    };
    const out: { label: string; albums: Album[] }[] = [];
    for (const a of shown) {
      const l = label(a.added);
      const prev = out[out.length - 1];
      if (prev?.label === l) prev.albums.push(a);
      else out.push({ label: l, albums: [a] });
    }
    return out;
  });
</script>

<div class="page">
  <header>
    <div>
      <h1 class="page-title">{recent ? "Recently Added" : "Albums"}</h1>
      <p class="secondary count">{app.albums.length} albums</p>
    </div>
    {#if !recent}
      <div class="tools">
        <button class="pill ghost" onclick={() => app.shuffleAlbums(app.albums)} disabled={!app.albums.length} title="A random album, whole and in order, then another: every album once">
          <Icon name="shuffle" size={15} />Shuffle albums
        </button>
        <button class="pill ghost" onclick={() => app.surpriseAlbum(app.albums)} disabled={!app.albums.length} title="Play one album, picked at random">
          <Icon name="sparkle" size={15} />Surprise me
        </button>
        <label class="sort"><span>Sort by</span><Select label="Sort albums" options={sorts} value={sort} onchange={(v) => (sort = v)} width="190px" /></label>
      </div>
    {/if}
  </header>

  {#if recent}
    {#each groups as g (g.label)}
      <h2 class="section-title group">{g.label}</h2>
      <div class="grid">
        {#each g.albums as album (album.id)}<AlbumCard {album} />{/each}
      </div>
    {/each}
  {:else}
    <div class="grid">
      {#each shown as album (album.id)}<AlbumCard {album} />{/each}
    </div>
  {/if}
</div>

<style>
  header {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 20px;
    margin-bottom: 26px;
  }
  .count {
    margin: 6px 0 0;
    font-size: 13px;
  }
  .tools {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 10px;
  }
  .sort {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    color: var(--text-2);
  }
  .group {
    margin: 8px 0 16px;
  }
  .group ~ .group {
    margin-top: 36px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 30px 24px;
  }
  /* Phones: two albums per row. */
  @media (max-width: 720px) {
    .grid {
      grid-template-columns: repeat(2, minmax(0, 1fr)) !important;
      gap: 20px 14px !important;
    }
  }
  @media (max-width: 720px) {
    .sort > span {
      display: none;
    }
  }
</style>

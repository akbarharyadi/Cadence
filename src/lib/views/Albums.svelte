<script lang="ts">
  import AlbumCard from "$lib/components/AlbumCard.svelte";
  import Select from "$lib/components/Select.svelte";
  import { app } from "$lib/store.svelte";
  import type { Album } from "$lib/api";

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
    for (const a of sorted) {
      const l = label(a.added);
      if (out.at(-1)?.label === l) out.at(-1)!.albums.push(a);
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
      <label class="sort"><span>Sort by</span><Select label="Sort albums" options={sorts} value={sort} onchange={(v) => (sort = v)} width="190px" /></label>
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
      {#each sorted as album (album.id)}<AlbumCard {album} />{/each}
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
</style>

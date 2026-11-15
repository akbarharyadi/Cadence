<script lang="ts">
  // Discover › New from artists you own: releases of the last weeks (and the next) by artists in the
  // library, from ListenBrainz's fresh releases, asked once a day when this page opens. Covers load
  // only for rows on screen. It needs no ListenBrainz account.
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { api, STORES, type NewRelease, type NewReleases } from "#lib/api.js";

  const FIRST = 8;
  let data = $state<NewReleases | null>(null);
  let all = $state(false);
  let list = $state<HTMLElement>();
  /** Rows that have been on screen: only they load a cover. */
  let seen = $state<Record<string, boolean>>({});
  let broken = $state<Record<string, boolean>>({});

  onMount(() => {
    let alive = true;
    // What was found last time at once, then today's ask (it only goes to the network when it's due).
    api.newReleasesCached().then((d) => alive && d && (data = d)).catch(() => {});
    api.newReleases().then((d) => alive && d && (data = d)).catch(() => {});
    return () => {
      alive = false;
    };
  });

  const items = $derived(data?.items ?? []);
  const shown = $derived(all ? items : items.slice(0, FIRST));

  // One observer for the list: a row's cover is asked for once the row first comes into view.
  $effect(() => {
    const root = list;
    // (Rows come and go with "Show all".)
    void shown.length;
    if (!root) return;
    const rows = Array.from(root.querySelectorAll<HTMLElement>("[data-key]"));
    if (typeof IntersectionObserver === "undefined") {
      for (const r of rows) seen[r.dataset.key ?? ""] = true;
      return;
    }
    const watch = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          const key = (e.target as HTMLElement).dataset.key ?? "";
          if (e.isIntersecting && key) {
            seen[key] = true;
            watch.unobserve(e.target);
          }
        }
      },
      { rootMargin: "120px" },
    );
    for (const r of rows) if (!seen[r.dataset.key ?? ""]) watch.observe(r);
    return () => watch.disconnect();
  });

  const day = (d: string) => {
    const t = Date.parse(`${d}T00:00:00`);
    return Number.isNaN(t) ? d : new Date(t).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
  };
  function buy(r: NewRelease, url: (q: string) => string) {
    void openUrl(url(encodeURIComponent(`${r.artist} ${r.title}`)));
  }
</script>

{#if data && (items.length > 0 || data.enabled || data.error)}
  <section class="fresh" aria-label="New from artists you own">
    <h2 class="section-title">New from artists you own</h2>
    {#if items.length > 0}
      <ul class="rows" bind:this={list}>
        {#each shown as r (r.key)}
          <li class="row" data-key={r.key}>
            <div class="cover">
              {#if r.cover && seen[r.key] && !broken[r.key]}
                <img src={r.cover} alt="" loading="lazy" onerror={() => (broken[r.key] = true)} />
              {/if}
            </div>
            <div class="info">
              <div class="top">
                <span class="title" title={r.title}>{r.title}</span>
                {#if r.kind}<span class="kind">{r.kind}</span>{/if}
                {#if r.upcoming}<span class="kind soon">Not out yet</span>{/if}
              </div>
              <div class="artist">{r.artist}{#if r.date}<span class="date"> · {r.upcoming ? "out " : ""}{day(r.date)}</span>{/if}</div>
              <div class="stores">
                <span class="buy">Buy on</span>
                {#each STORES as s (s.name)}
                  <button class="store" onclick={() => buy(r, s.url)}>{s.name}</button>
                {/each}
              </div>
            </div>
          </li>
        {/each}
      </ul>
      {#if items.length > FIRST}
        <button class="pill ghost more" onclick={() => (all = !all)}>{all ? "Show fewer" : `Show all ${items.length}`}</button>
      {/if}
    {:else if data.error}
      <p class="note">{data.error}</p>
    {:else if data.fetched > 0}
      <p class="note">Nothing new from the artists in your library in the last month.</p>
    {:else}
      <p class="note">Looking for new releases by your artists…</p>
    {/if}
  </section>
{/if}

<style>
  .fresh {
    margin: 0 0 30px;
  }
  .section-title {
    margin: 0 0 14px;
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(380px, 1fr));
    gap: 10px;
  }
  .row {
    display: flex;
    gap: 12px;
    padding: 10px;
    border-radius: var(--r-md);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    min-width: 0;
  }
  .cover {
    flex: none;
    width: 56px;
    height: 56px;
    border-radius: var(--r-sm);
    background: var(--active);
    overflow: hidden;
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .info {
    min-width: 0;
    flex: 1;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .title {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind {
    flex: none;
    font-size: 11px;
    padding: 1px 7px;
    border-radius: 999px;
    color: var(--text-2);
    box-shadow: inset 0 0 0 1px var(--line-strong);
  }
  .kind.soon {
    color: var(--accent);
  }
  .artist {
    color: var(--text-2);
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .stores {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 6px;
    margin-top: 6px;
    font-size: 12px;
  }
  .buy {
    color: var(--text-3);
  }
  .store {
    padding: 1px 8px;
    border-radius: 999px;
    color: var(--text-2);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .store:hover {
    color: var(--text);
    background: var(--active);
  }
  .more {
    margin-top: 12px;
  }
  .note {
    margin: 0;
    color: var(--text-2);
    font-size: 13.5px;
  }
</style>

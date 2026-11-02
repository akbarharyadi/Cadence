<script lang="ts">
  // Discover: new songs (not in the library) found on the web by the AI DJ, for what the listener
  // plays this month, an artist, a genre, or anything said in words. Each shows how similar it is
  // and to which of their songs, why, and where to buy it.
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { STORES, type DiscoverBasis, type NewSong, type PlayCount } from "#lib/api.js";
  import Icon from "#lib/components/Icon.svelte";
  import Segmented from "#lib/components/Segmented.svelte";
  import { app } from "#lib/store.svelte.js";

  let basis = $state<DiscoverBasis>("month");
  let text = $state("");

  onMount(async () => {
    // Today's picks are ready (or found now, once a day); the last search is shown on its tab.
    if (!app.daily) app.loadDaily(false);
    await app.loadDiscover();
  });

  /** This month's tab shows today's picks; the others, the last search. */
  const shown = $derived(basis === "month" ? app.daily : app.discover?.basis === basis ? app.discover : null);
  const loading = $derived(basis === "month" ? app.dailyState === "loading" : app.discoverState === "loading");
  const failed = $derived(basis === "month" ? (app.dailyState === "error" ? app.dailyError : "") : app.discoverState === "error" ? app.discoverError : "");
  const p = $derived(app.profile);

  /** This month's names first, then all-time ones, without repeats. */
  function merged(a: PlayCount[], b: PlayCount[]): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const c of [...a, ...b]) {
      const k = c.name.toLowerCase();
      if (!seen.has(k)) {
        seen.add(k);
        out.push(c.name);
      }
    }
    return out.slice(0, 16);
  }

  const chips = $derived(
    !p ? [] : basis === "artist" ? merged(p.monthArtists, p.artists) : basis === "genre" ? merged(p.monthGenres, p.genres) : [],
  );

  const monthWords = $derived.by(() => {
    if (!p || (!p.monthArtists.length && !p.monthGenres.length)) return "";
    const artists = p.monthArtists.slice(0, 4).map((c) => c.name).join(", ");
    const genres = p.monthGenres.slice(0, 3).map((c) => c.name).join(", ");
    return [artists, genres].filter(Boolean).join(" · ");
  });

  const heading = $derived.by(() => {
    const d = shown;
    if (!d) return "";
    if (d.basis === "artist") return `For fans of ${d.value}`;
    if (d.basis === "genre") return `${d.value} for you`;
    if (d.basis === "search") return `“${d.value}”`;
    return "Today’s picks for you";
  });

  const placeholder = $derived(
    basis === "artist" ? "Any artist" : basis === "genre" ? "Any genre" : "Describe what you’d like to hear, e.g. city pop from this year",
  );

  function find(value = text) {
    if (basis === "month") app.loadDaily(true);
    else app.recommend(basis, value);
  }

  /** A 0–100 rating as stars out of 5, one decimal. */
  const stars = (r: number) => (r / 20).toFixed(1);

  function pick(name: string) {
    text = name;
    find(name);
  }

  function buy(d: NewSong, url: (q: string) => string) {
    openUrl(url(encodeURIComponent(`${d.artist} ${d.title}`)));
  }
</script>

<div class="page discover">
  <h1 class="page-title">Discover</h1>
  <p class="sub">New songs for you, found on the web by the AI DJ: how close each one is to what you asked for, and where to buy it.</p>

  {#if !app.aiReady}
    <div class="setup">
      <p>Turn on the AI DJ in Settings and add your API key (Claude, or another service) to discover new music.</p>
      <button class="pill" onclick={() => app.go({ name: "settings" })}><Icon name="settings" size={15} />Open Settings</button>
    </div>
  {:else}
    <div class="controls">
      <Segmented
        label="Based on"
        options={[
          { value: "month", label: "This month" },
          { value: "artist", label: "An artist" },
          { value: "genre", label: "A genre" },
          { value: "search", label: "Your words" },
        ]}
        value={basis}
        onchange={(v) => {
          basis = v;
          text = "";
        }}
      />

      {#if basis === "month"}
        <div class="row">
          <p class="hint">
            {#if monthWords}Your last 30 days: {monthWords}.{:else}Nothing played in the last 30 days yet, so it goes by all your listening.{/if}
          </p>
          <button class="pill ghost" onclick={() => find()} disabled={loading}><Icon name="refresh" size={15} />New picks</button>
        </div>
      {:else}
        <form class="ask" onsubmit={(e) => { e.preventDefault(); find(); }}>
          <input class="field" bind:value={text} {placeholder} aria-label={placeholder} spellcheck="false" />
          <button class="pill" type="submit" disabled={!text.trim() || loading}><Icon name="search" size={15} />Find</button>
        </form>
        {#if chips.length}
          <div class="chips" aria-label={basis === "artist" ? "Your top artists" : "Your top genres"}>
            {#each chips as c (c)}
              <button class="chip" class:on={text === c} onclick={() => pick(c)} disabled={loading}>{c}</button>
            {/each}
          </div>
        {/if}
      {/if}
    </div>

    {#if loading}
      <p class="note"><span class="spin" aria-hidden="true"></span>The DJ is searching the web for songs you’d like. This can take a minute.</p>
    {:else if failed}
      <p class="error">{failed}</p>
    {/if}

    {#if shown?.picks.length}
      <h2 class="section-title">{heading}</h2>
      <ul class="results" class:dim={loading}>
        {#each shown.picks as d (d.artist + d.title)}
          <li class="result">
            <div class="art" class:artist={d.imageIsArtist}>
              {#if d.image}<img src={d.image} alt="" loading="lazy" />{:else}<Icon name="song" size={28} />{/if}
            </div>
            <div class="info">
              <div class="top">
                <span class="title" title={d.title}>{d.title}</span>
                {#if d.similarity}<span class="match" title="How close it is to what you asked for">{d.similarity}% match</span>{/if}
                {#if d.rating}<span class="rating" title="How well listeners and critics rate it">★ {stars(d.rating)}</span>{/if}
              </div>
              <div class="artist" title={d.album ? `${d.artist} · ${d.album}` : d.artist}>{d.artist}{#if d.album}<span class="album"> · {d.album}</span>{/if}</div>
              {#if d.like}<div class="like"><Icon name="wave" size={13} />Like {d.like}</div>{/if}
              {#if d.why}<p class="why">{d.why}</p>{/if}
              <div class="stores">
                <span class="buy">Buy on</span>
                {#each STORES as s (s.name)}
                  <button class="store" onclick={() => buy(d, s.url)}>{s.name}</button>
                {/each}
              </div>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<style>
  .sub {
    margin: 6px 0 26px;
    color: var(--text-2);
    max-width: 70ch;
  }
  .setup p {
    color: var(--text-2);
    margin: 0 0 14px;
  }
  .controls {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 14px;
    margin-bottom: 26px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px 18px;
  }
  .hint {
    margin: 0;
    color: var(--text-2);
    font-size: 13.5px;
  }
  .ask {
    display: flex;
    gap: 10px;
    width: min(620px, 100%);
  }
  .ask .field {
    flex: 1;
    min-width: 0;
    font-size: 14px;
    padding: 8px 14px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    max-width: 860px;
  }
  .chip {
    padding: 5px 12px;
    border-radius: 999px;
    font-size: 13px;
    color: var(--text-2);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    transition: background 0.15s, color 0.15s;
  }
  .chip:hover:not(:disabled) {
    color: var(--text);
    background: var(--active);
  }
  .chip.on {
    color: var(--ink);
    background: var(--accent);
    box-shadow: none;
  }
  .note {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-2);
    margin: 0 0 22px;
  }
  .spin {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .error {
    color: var(--bad);
    margin: 0 0 22px;
  }
  .section-title {
    margin: 0 0 14px;
  }
  .results {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(420px, 1fr));
    gap: 12px;
    transition: opacity 0.2s;
  }
  .results.dim {
    opacity: 0.5;
  }
  .result {
    display: flex;
    gap: 14px;
    padding: 12px;
    border-radius: var(--r-lg);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    min-width: 0;
  }
  .art {
    width: 88px;
    height: 88px;
    flex: none;
    border-radius: 10px;
    overflow: hidden;
    display: grid;
    place-items: center;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .art.artist {
    border-radius: 50%;
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .title {
    font-weight: 600;
    font-size: 15px;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .match {
    flex: none;
    padding: 1px 8px;
    border-radius: 999px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }
  .rating {
    flex: none;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--gold);
  }
  .artist {
    font-size: 13px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .album {
    color: var(--text-3);
  }
  .like {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12.5px;
    color: var(--text-2);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .like :global(svg) {
    flex: none;
    color: var(--accent);
  }
  .why {
    margin: 2px 0 0;
    font-size: 12.5px;
    color: var(--text-3);
    line-height: 1.45;
  }
  .stores {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
  }
  .buy {
    font-size: 11.5px;
    color: var(--text-3);
    margin-right: 2px;
  }
  .store {
    padding: 2px 8px;
    border-radius: 999px;
    font-size: 11.5px;
    color: var(--text-2);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .store:hover {
    color: var(--text);
    background: var(--active);
  }
  @media (max-width: 720px) {
    .results {
      grid-template-columns: minmax(0, 1fr);
    }
    .art {
      width: 72px;
      height: 72px;
    }
  }
</style>

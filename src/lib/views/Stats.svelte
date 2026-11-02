<script lang="ts">
  // Your Stats: what you play most — songs, artists, albums — counting plays heard to the end,
  // for the last week, month, year or all time; how long you listened, and how often you skip.
  import { api, artUrl, type ListeningStats, type StatItem, type StatsRange } from "#lib/api.js";
  import Icon from "#lib/components/Icon.svelte";
  import Segmented from "#lib/components/Segmented.svelte";
  import { app } from "#lib/store.svelte.js";

  let range = $state<StatsRange>("month");
  let stats = $state<ListeningStats | null>(null);
  let tab = $state<"songs" | "artists" | "albums">("songs");

  $effect(() => {
    const r = range;
    // Again when a play is counted, so the page stays current while music plays.
    void app.listening;
    api.listeningStats(r).then((s) => r === range && (stats = s));
  });

  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const hours = (m: number) => (m >= 120 ? `${Math.round(m / 60)} h` : m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m} min`);
  const finishedShare = $derived(stats && stats.plays ? Math.round((stats.finished / stats.plays) * 100) : 0);
  const rows = $derived(!stats ? [] : tab === "songs" ? stats.topSongs : tab === "artists" ? stats.topArtists : stats.topAlbums);
  const best = $derived(Math.max(1, ...rows.map((r) => r.finished)));
  const peak = $derived(Math.max(1, ...(stats?.timeline ?? [0])));

  /** The bar's label: the day (week), its date (month), or the month (year). */
  function label(i: number): string {
    if (!stats) return "";
    const n = stats.timeline.length;
    const step = stats.range === "year" ? 30 : 1;
    const d = new Date(Date.now() - (n - 1 - i) * step * 86_400_000);
    if (stats.range === "week") return d.toLocaleDateString(undefined, { weekday: "short" });
    if (stats.range === "year") return d.toLocaleDateString(undefined, { month: "short" });
    return d.getDate().toString();
  }

  function art(r: StatItem): string | null {
    if (tab === "songs") {
      const t = app.trackById.get(r.id);
      return artUrl(t ? app.albumById.get(t.albumId)?.artSmall : null) || null;
    }
    if (tab === "albums") return artUrl(app.albumById.get(r.id)?.artSmall) || null;
    return null;
  }

  function open(r: StatItem) {
    if (tab === "songs") {
      const t = app.trackById.get(r.id);
      if (t) app.go({ name: "album", id: t.albumId });
    } else if (tab === "albums") app.go({ name: "album", id: r.id });
    else app.go({ name: "artist", artist: r.name });
  }

  function play(r: StatItem) {
    if (tab === "songs") {
      const t = app.trackById.get(r.id);
      if (t) app.playTracks([t]);
    } else if (tab === "albums") {
      const a = app.albumById.get(r.id);
      if (a) app.playAlbum(a);
    } else {
      app.playTracks(app.tracks.filter((t) => t.artist.toLowerCase() === r.name.toLowerCase()));
    }
  }
</script>

<div class="page stats">
  <div class="head">
    <div>
      <h1 class="page-title">Your Stats</h1>
      <p class="sub">What you play most, counting songs heard to the end.</p>
    </div>
    <Segmented
      label="Time range"
      options={[
        { value: "week", label: "Week" },
        { value: "month", label: "Month" },
        { value: "year", label: "Year" },
        { value: "all", label: "All time" },
      ]}
      value={range}
      onchange={(v) => (range = v)}
    />
  </div>

  {#if stats}
    <div class="tiles">
      <div class="tile"><span class="num">{hours(stats.minutes)}</span><span class="what">listening</span></div>
      <div class="tile"><span class="num">{stats.finished}</span><span class="what">{stats.finished === 1 ? "song" : "songs"} played to the end</span></div>
      <div class="tile"><span class="num">{finishedShare}%</span><span class="what">finished, {stats.skipped} skipped</span></div>
      <div class="tile">
        <span class="num">{stats.artists}</span>
        <span class="what">{stats.artists === 1 ? "artist" : "artists"} · {plural(stats.albums, "album")} · {plural(stats.songs, "song")}</span>
      </div>
    </div>

    {#if stats.timeline.length && stats.minutes > 0}
      <div class="chart" aria-label="Listening time">
        {#each stats.timeline as m, i (i)}
          <div class="col" title={`${label(i)}: ${hours(m)}`}>
            <div class="bar" style="height: {Math.max(m ? 4 : 0, (m / peak) * 100)}%"></div>
            {#if stats.range !== "month" || i % 5 === 4 || i === stats.timeline.length - 1}<span class="day">{label(i)}</span>{/if}
          </div>
        {/each}
      </div>
    {/if}

    {#if stats.plays === 0}
      <p class="empty">Nothing played {range === "week" ? "this week" : range === "month" ? "this month" : range === "year" ? "this year" : "yet"}. Play some music and it shows up here.</p>
    {:else}
      <div class="tabs">
        <Segmented
          label="Top"
          options={[
            { value: "songs", label: "Songs" },
            { value: "artists", label: "Artists" },
            { value: "albums", label: "Albums" },
          ]}
          value={tab}
          onchange={(v) => (tab = v)}
        />
      </div>
      <ol class="top">
        {#each rows.slice(0, 25) as r, i (r.id)}
          {@const a = art(r)}
          <li class="row">
            <span class="rank">{i + 1}</span>
            <button class="pic" class:round={tab === "artists"} onclick={() => play(r)} aria-label={`Play ${r.name}`} title="Play">
              {#if a}<img src={a} alt="" loading="lazy" />{:else}<Icon name={tab === "artists" ? "artist" : tab === "albums" ? "album" : "song"} size={18} />{/if}
              <span class="hover"><Icon name="play" size={14} /></span>
            </button>
            <button class="name" onclick={() => open(r)}>
              <span class="title">{r.name}</span>
              {#if r.sub}<span class="by">{r.sub}</span>{/if}
            </button>
            <div class="meter" aria-hidden="true"><div class="fill" style="width: {(r.finished / best) * 100}%"></div></div>
            <span class="count">
              <span><strong>{r.finished}</strong> to the end</span>
              <span class="of">of {r.plays} · {hours(r.minutes)}</span>
            </span>
          </li>
        {/each}
      </ol>
    {/if}
  {/if}
</div>

<style>
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 16px 24px;
    margin-bottom: 26px;
  }
  .sub {
    margin: 6px 0 0;
    color: var(--text-2);
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 12px;
    margin-bottom: 26px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 16px 18px;
    border-radius: var(--r-lg);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .num {
    font-family: var(--display);
    font-size: 26px;
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--text);
  }
  .what {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .chart {
    display: flex;
    align-items: flex-end;
    gap: 4px;
    height: 140px;
    padding: 0 2px 22px;
    margin-bottom: 30px;
  }
  .col {
    position: relative;
    flex: 1;
    height: 100%;
    display: flex;
    align-items: flex-end;
  }
  .bar {
    width: 100%;
    border-radius: 4px 4px 2px 2px;
    background: var(--accent);
    opacity: 0.85;
  }
  .col:hover .bar {
    opacity: 1;
  }
  .day {
    position: absolute;
    bottom: -20px;
    left: 50%;
    transform: translateX(-50%);
    font-size: 11px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .empty {
    color: var(--text-2);
  }
  .tabs {
    margin-bottom: 14px;
  }
  .top {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: grid;
    grid-template-columns: 28px 44px minmax(0, 1.4fr) minmax(80px, 1fr) 150px;
    align-items: center;
    gap: 14px;
    padding: 6px 10px;
    border-radius: 8px;
  }
  .row:hover {
    background: var(--hover);
  }
  .rank {
    text-align: right;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .pic {
    position: relative;
    width: 44px;
    height: 44px;
    border-radius: 6px;
    overflow: hidden;
    display: grid;
    place-items: center;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .pic.round {
    border-radius: 50%;
  }
  .pic img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .hover {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.5);
    color: white;
    opacity: 0;
    transition: opacity 0.15s;
  }
  .pic:hover .hover {
    opacity: 1;
  }
  .name {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    min-width: 0;
    text-align: left;
  }
  .title {
    max-width: 100%;
    font-size: 14px;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name:hover .title {
    text-decoration: underline;
  }
  .by {
    max-width: 100%;
    font-size: 12.5px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meter {
    height: 6px;
    border-radius: 3px;
    background: var(--hover);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 3px;
    background: var(--accent);
  }
  .count {
    font-size: 12.5px;
    color: var(--text-2);
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    font-variant-numeric: tabular-nums;
  }
  .count strong {
    color: var(--text);
    font-size: 14px;
  }
  .of {
    color: var(--text-3);
    font-size: 11.5px;
  }
  @media (max-width: 720px) {
    .row {
      grid-template-columns: 22px 40px minmax(0, 1fr) 96px;
    }
    .meter {
      display: none;
    }
  }
</style>

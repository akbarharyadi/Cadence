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

  let error = $state("");
  $effect(() => {
    const r = range;
    // Again when a play is counted, so the page stays current while music plays.
    void app.listening;
    api
      .listeningStats(r)
      .then((s) => {
        if (r !== range) return;
        stats = s;
        error = "";
      })
      .catch((e) => {
        if (r === range) error = String(e);
      });
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

  // ---------- sections over the whole history (each hidden without enough to show) ----------
  const sum = (v: number[] | undefined) => (v ?? []).reduce((a, b) => a + b, 0);
  const onThisDay = $derived(stats?.onThisDay ?? []);
  const forgotten = $derived(stats?.forgotten ?? []);
  const clock = $derived(stats?.clock?.length === 24 && sum(stats.clock) >= 20 ? stats.clock : null);
  const weekdays = $derived(stats?.weekdays?.length === 7 && sum(stats.weekdays) >= 20 ? stats.weekdays : null);
  const streak = $derived(stats?.streak && stats.streak.longest >= 2 ? stats.streak : null);
  const discovery = $derived(stats?.discovery && stats.discovery.length >= 3 && stats.discovery.some((m) => m.count > 0) ? stats.discovery : null);
  const firstSong = $derived(stats?.firstSong ?? null);
  const wholeAlbums = $derived(stats?.completeAlbums ?? []);
  const decades = $derived((stats?.decades?.length ?? 0) >= 2 ? stats!.decades! : null);
  // Hidden when everything played is in one language (or the backend is older).
  const languages = $derived(Array.isArray(stats?.languages) && stats.languages.length >= 2 ? stats.languages : null);
  const peakOf = (v: number[]) => Math.max(1, ...v);
  const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
  const inRange = $derived(range === "week" ? "this week" : range === "month" ? "this month" : range === "year" ? "this year" : "all time");
  const monthLabel = (ym: string) => {
    const [y, m] = ym.split("-").map(Number);
    return y && m ? new Date(y, m - 1, 1).toLocaleDateString(undefined, { month: "short" }) : ym;
  };
  const fullDate = (ymd: string) => {
    const [y, m, d] = ymd.split("-").map(Number);
    return y && m && d ? new Date(y, m - 1, d).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }) : ymd;
  };
  const monthYear = (secs: number) => new Date(secs * 1000).toLocaleDateString(undefined, { month: "short", year: "numeric" });
  const today = new Date().toLocaleDateString(undefined, { day: "numeric", month: "long" });

  function songArt(id: string): string | null {
    const t = app.trackById.get(id);
    return artUrl(t ? app.albumById.get(t.albumId)?.artSmall : null) || null;
  }
  function openSong(id: string) {
    const t = app.trackById.get(id);
    if (t) app.go({ name: "album", id: t.albumId });
  }
  function playSong(id: string) {
    const t = app.trackById.get(id);
    if (t) app.playTracks([t]);
  }
  /** "Play these": the section's songs, in its order (each once). */
  function playAll(ids: string[]) {
    const seen = new Set<string>();
    const tracks = ids.flatMap((id) => {
      const t = seen.has(id) ? undefined : app.trackById.get(id);
      seen.add(id);
      return t ? [t] : [];
    });
    if (tracks.length) app.playTracks(tracks);
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

  {#if error && (!stats || stats.range !== range)}
    <p class="empty failed">Couldn't read your listening history: {error}</p>
  {:else if !stats}
    <div class="tiles" aria-busy="true" aria-label="Loading">
      {#each [0, 1, 2, 3] as i (i)}<div class="tile ghost"><span class="num">–</span><span class="what">&nbsp;</span></div>{/each}
    </div>
  {:else}
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

    {#if streak || firstSong}
      <div class="tiles more" aria-label="Streaks">
        {#if streak}
          <div class="tile">
            <span class="num">{plural(streak.current, "day")}</span>
            <span class="what">{streak.current ? "in a row, up to today" : "no streak going on now"}</span>
          </div>
          <div class="tile">
            <span class="num">{plural(streak.longest, "day")}</span>
            <span class="what">longest streak, {fullDate(streak.longestFrom)} – {fullDate(streak.longestTo)}</span>
          </div>
        {/if}
        {#if firstSong}
          <button class="tile first" onclick={() => openSong(firstSong.id)} title="Open its album">
            <span class="what">First song of {new Date(firstSong.at * 1000).getFullYear()}</span>
            <span class="first-name">{firstSong.name}</span>
            <span class="what">{firstSong.sub}</span>
          </button>
        {/if}
      </div>
    {/if}

    {#if onThisDay.length}
      <section class="block" aria-label="On this day">
        <div class="block-head">
          <div>
            <h2 class="section-title">On this day</h2>
            <p class="sub">What you played on {today} in earlier years.</p>
          </div>
          <button class="pill ghost" onclick={() => playAll(onThisDay.map((d) => d.id))}><Icon name="play" size={14} />Play these</button>
        </div>
        <ul class="list">
          {#each onThisDay as d (`${d.year}-${d.id}`)}
            {@const a = songArt(d.id)}
            <li class="item">
              <button class="pic" onclick={() => playSong(d.id)} aria-label={`Play ${d.name}`} title="Play">
                {#if a}<img src={a} alt="" loading="lazy" />{:else}<Icon name="song" size={16} />{/if}
                <span class="hover"><Icon name="play" size={13} /></span>
              </button>
              <button class="name" onclick={() => openSong(d.id)}>
                <span class="title">{d.name}</span>
                <span class="by">{d.sub}</span>
              </button>
              <span class="count"><span><strong>{d.year}</strong></span><span class="of">{plural(d.plays, "play")}</span></span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if forgotten.length}
      <section class="block" aria-label="Forgotten favourites">
        <div class="block-head">
          <div>
            <h2 class="section-title">Forgotten favourites</h2>
            <p class="sub">Songs you played a lot, loved or rated highly, and haven't played for half a year.</p>
          </div>
          <button class="pill ghost" onclick={() => playAll(forgotten.map((f) => f.id))}><Icon name="play" size={14} />Play these</button>
        </div>
        <ul class="list">
          {#each forgotten as f (f.id)}
            {@const a = songArt(f.id)}
            <li class="item">
              <button class="pic" onclick={() => playSong(f.id)} aria-label={`Play ${f.name}`} title="Play">
                {#if a}<img src={a} alt="" loading="lazy" />{:else}<Icon name="song" size={16} />{/if}
                <span class="hover"><Icon name="play" size={13} /></span>
              </button>
              <button class="name" onclick={() => openSong(f.id)}>
                <span class="title">{f.name}{#if f.loved}<span class="loved" aria-label="Favorite"> ♥</span>{/if}</span>
                <span class="by">{f.sub}</span>
              </button>
              <span class="count">
                <span><strong>{f.plays}</strong> to the end</span>
                <span class="of">last {monthYear(f.last)}{f.rating ? ` · ${"★".repeat(f.rating)}` : ""}</span>
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if clock || weekdays}
      <section class="block" aria-label="When you listen">
        <h2 class="section-title">When you listen</h2>
        <p class="sub">Plays by hour of the day and by weekday, {inRange}.</p>
        <div class="when">
          {#if clock}
            {@const top = peakOf(clock)}
            <div class="chart small" aria-label="Plays by hour">
              {#each clock as n, h (h)}
                <div class="col" title={`${String(h).padStart(2, "0")}:00: ${plural(n, "play")}`}>
                  <div class="bar" style="height: {Math.max(n ? 4 : 0, (n / top) * 100)}%"></div>
                  {#if h % 6 === 0}<span class="day">{String(h).padStart(2, "0")}:00</span>{/if}
                </div>
              {/each}
            </div>
          {/if}
          {#if weekdays}
            {@const top = peakOf(weekdays)}
            <ul class="bars" aria-label="Plays by weekday">
              {#each weekdays as n, i (i)}
                <li>
                  <span class="lbl">{WEEKDAYS[i]}</span>
                  <div class="meter"><div class="fill" style="width: {(n / top) * 100}%"></div></div>
                  <span class="n">{n}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      </section>
    {/if}

    {#if discovery}
      {@const top = peakOf(discovery.map((m) => m.count))}
      <section class="block" aria-label="New artists">
        <h2 class="section-title">New artists</h2>
        <p class="sub">Artists you heard for the first time, per month.</p>
        <div class="chart small">
          {#each discovery as m (m.month)}
            <div class="col" title={`${monthLabel(m.month)}: ${plural(m.count, "new artist")}`}>
              <div class="bar" style="height: {Math.max(m.count ? 4 : 0, (m.count / top) * 100)}%"></div>
              <span class="day">{monthLabel(m.month)}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if wholeAlbums.length}
      <section class="block" aria-label="Albums heard through">
        <h2 class="section-title">Albums heard through</h2>
        <p class="sub">Albums you played (almost) all the way through in one go, {inRange}.</p>
        <ul class="list">
          {#each wholeAlbums as al (al.id)}
            {@const a = artUrl(app.albumById.get(al.id)?.artSmall) || null}
            {@const album = app.albumById.get(al.id)}
            <li class="item">
              <button class="pic" onclick={() => album && app.playAlbum(album)} aria-label={`Play ${al.name}`} title="Play">
                {#if a}<img src={a} alt="" loading="lazy" />{:else}<Icon name="album" size={16} />{/if}
                <span class="hover"><Icon name="play" size={13} /></span>
              </button>
              <button class="name" onclick={() => app.go({ name: "album", id: al.id })}>
                <span class="title">{al.name}</span>
                <span class="by">{al.sub}</span>
              </button>
              <span class="count"><span><strong>{al.finished}</strong> {al.finished === 1 ? "time" : "times"}</span><span class="of">{hours(al.minutes)}</span></span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if decades}
      {@const top = peakOf(decades.map((d) => d.plays))}
      <section class="block" aria-label="Decades">
        <h2 class="section-title">By decade</h2>
        <p class="sub">Plays by the year of the songs, {inRange}.</p>
        <ul class="bars">
          {#each decades as d (d.decade)}
            <li>
              <span class="lbl">{d.decade}s</span>
              <div class="meter"><div class="fill" style="width: {(d.plays / top) * 100}%"></div></div>
              <span class="n">{plural(d.plays, "play")}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if languages}
      {@const top = peakOf(languages.map((l) => l.plays))}
      <section class="block" aria-label="Languages">
        <h2 class="section-title">By language</h2>
        <p class="sub">Plays by the language the songs are sung in, {inRange}. Songs Cadence can't tell are left out.</p>
        <ul class="bars wide">
          {#each languages as l (l.code)}
            <li>
              <span class="lbl">{l.name}</span>
              <div class="meter"><div class="fill" style="width: {(l.plays / top) * 100}%"></div></div>
              <span class="n">{plural(l.plays, "play")}</span>
            </li>
          {/each}
        </ul>
      </section>
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
  .empty.failed {
    color: var(--warn);
  }
  .tile.ghost .num {
    color: var(--text-3);
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
  /* ---------- sections over the whole history ---------- */
  .tiles.more {
    margin-top: 34px;
  }
  .tile.first {
    align-items: flex-start;
    text-align: left;
    min-width: 0;
  }
  .tile.first:hover {
    background: var(--active);
  }
  .first-name {
    max-width: 100%;
    font-family: var(--display);
    font-size: 17px;
    font-weight: 700;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .block {
    margin-top: 34px;
  }
  .block .sub {
    margin: 4px 0 14px;
    font-size: 13px;
  }
  .block-head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
  }
  .block-head .pill {
    flex: none;
    margin-bottom: 14px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 2px 18px;
  }
  .item {
    display: grid;
    grid-template-columns: 40px minmax(0, 1fr) auto;
    align-items: center;
    gap: 12px;
    padding: 5px 8px;
    border-radius: 8px;
  }
  .item:hover {
    background: var(--hover);
  }
  .item .pic {
    width: 40px;
    height: 40px;
  }
  .loved {
    color: var(--accent);
  }
  .when {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(200px, 1fr);
    gap: 28px;
    align-items: end;
  }
  .chart.small {
    height: 110px;
    margin-bottom: 8px;
  }
  .bars {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 560px;
  }
  .bars li {
    display: grid;
    grid-template-columns: 52px minmax(60px, 1fr) auto;
    align-items: center;
    gap: 12px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .bars.wide li {
    grid-template-columns: 84px minmax(60px, 1fr) auto;
  }
  .bars .n {
    min-width: 40px;
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
  }
  @media (max-width: 720px) {
    .when {
      grid-template-columns: 1fr;
    }
    /* (The top list hides its meters on a narrow screen; these are the whole point.) */
    .bars .meter {
      display: block !important;
    }
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

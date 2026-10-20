<script lang="ts">
  import { api, artUrl, formatDuration, STORES, type AiMix, type Album, type NewSong, type Track } from "$lib/api";
  import AlbumCard from "$lib/components/AlbumCard.svelte";
  import Carousel from "$lib/components/Carousel.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import QualityBadge from "$lib/components/QualityBadge.svelte";
  import { app, shuffled } from "$lib/store.svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";

  const newest = $derived([...app.albums].sort((a, b) => b.added - a.added));
  /** The album the page opens with: what's playing, else the last played, else the newest. */
  const hero = $derived<Album | undefined>(
    app.currentAlbum ?? app.albumById.get(app.recentlyPlayed[0] ?? "") ?? newest[0],
  );
  const heroState = $derived(app.currentAlbum ? "Now playing" : app.recentlyPlayed.length ? "Pick up where you left off" : "Newest in your library");
  const playedRow = $derived(app.recentlyPlayed.map((id) => app.albumById.get(id)).filter((a): a is Album => !!a && a.id !== hero?.id));
  const addedRow = $derived(newest.filter((a) => a.id !== hero?.id).slice(0, 30));
  const losslessShare = $derived(app.albums.length ? app.albums.filter((a) => a.quality.includes("Lossless")).length / app.albums.length : 0);
  const totalTime = $derived(app.tracks.reduce((s, t) => s + t.duration, 0));

  /** The covers a mix shows: its albums, most-represented first. */
  function mixArt(tracks: Track[]): string[] {
    const counts = new Map<string, number>();
    for (const t of tracks) counts.set(t.albumId, (counts.get(t.albumId) ?? 0) + 1);
    return [...counts.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([id]) => app.albumById.get(id)?.art)
      .filter((a): a is string => !!a)
      .slice(0, 3);
  }

  async function saveMix(name: string, tracks: Track[]) {
    const p = await app.createPlaylist(name, tracks);
    app.showToast(`Saved “${p.name}” as a playlist`);
  }

  const mixMenu = (e: MouseEvent, name: string, tracks: Track[]) =>
    app.openMenu(e, [
      { label: "Play", icon: "play", action: () => app.playTracks(tracks) },
      { label: "Shuffle", icon: "shuffle", action: () => app.playTracks(shuffled(tracks)) },
      { label: "Save as Playlist", icon: "playlist", action: () => saveMix(name, tracks) },
    ]);

  const aiTracks = (m: AiMix) => m.trackIds.map((id) => app.trackById.get(id)).filter((t): t is Track => !!t);

  // ---------- Ask the DJ ----------
  let request = $state("");
  let asking = $state(false);
  let answer = $state<AiMix | null>(null);
  let askError = $state("");
  const suggestions = ["Calm songs for late-night coding", "Something epic to focus", "Upbeat songs with vocals", "Quiet instrumentals"];

  async function ask(text = request) {
    text = text.trim();
    if (!text || asking) return;
    request = text;
    asking = true;
    askError = "";
    answer = null;
    try {
      answer = await api.aiAsk(text);
    } catch (e) {
      askError = String(e);
    } finally {
      asking = false;
    }
  }

  function shuffleAll() {
    app.playTracks(shuffled(app.tracks));
  }
</script>

{#snippet stores(d: NewSong)}
  <span class="stores">
    <span class="buy">Buy on</span>
    {#each STORES as s}
      <button class="store" onclick={() => openUrl(s.url(encodeURIComponent(`${d.artist} ${d.title}`)))}>{s.name}</button>
    {/each}
  </span>
{/snippet}

{#snippet newCard(d: NewSong)}
  <div class="new-card">
    <div class="new-art" class:artist={d.imageIsArtist}>
      {#if d.image}<img src={d.image} alt="" loading="lazy" />{:else}<Icon name="song" size={40} />{/if}
    </div>
    <div class="new-title" title={d.title}>{d.title}</div>
    <div class="new-artist" title={d.artist}>{d.artist}</div>
    {#if d.why}<p class="new-why">{d.why}</p>{/if}
    {@render stores(d)}
  </div>
{/snippet}

{#snippet tile(name: string, desc: string, tracks: Track[])}
  {@const art = mixArt(tracks)}
  <div class="mix" role="group" aria-label={name} oncontextmenu={(e) => mixMenu(e, name, tracks)}>
    <button class="mix-tile" onclick={() => app.playTracks(tracks)} aria-label="Play {name}">
      {#each art as a, i}<img class="layer l{i}" src={artUrl(a)} alt="" />{/each}
      <span class="mix-name">{name}</span>
      <span class="mix-play"><Icon name="play" size={18} /></span>
    </button>
    <p class="mix-desc">{desc}</p>
  </div>
{/snippet}

<div class="home">
  {#if hero}
    {@const playing = app.currentAlbum?.id === hero.id}
    <section class="hero">
      <button class="cover" onclick={() => app.go({ name: "album", id: hero.id })} aria-label="Open {hero.title}">
        {#if hero.art}<img src={artUrl(hero.art)} alt="" />{:else}<Icon name="album" size={72} />{/if}
      </button>
      <div class="info">
        <p class="state">{heroState}</p>
        <h1>{hero.title}</h1>
        <button class="artist" onclick={() => app.go({ name: "artist", artist: app.artistOf(hero.artist) })}>{hero.artist}</button>
        <div class="facts">
          <QualityBadge quality={hero.quality} detail={hero.format} />
          <span>{hero.trackIds.length} songs, {formatDuration(hero.duration)}</span>
        </div>
        <div class="actions">
          {#if playing}
            <button class="pill" onclick={() => app.togglePlay()}>
              <Icon name={app.status?.playing ? "pause" : "play"} size={15} />{app.status?.playing ? "Pause" : "Resume"}
            </button>
          {:else}
            <button class="pill" onclick={() => app.playAlbum(hero)}><Icon name="play" size={15} />Play album</button>
          {/if}
          <button class="pill ghost" onclick={shuffleAll}><Icon name="shuffle" size={15} />Shuffle everything</button>
        </div>
      </div>
    </section>
  {/if}

  {#if app.aiReady}
    <section class="dj">
      <h2 class="section-title">Ask the DJ</h2>
      <form class="ask" onsubmit={(e) => { e.preventDefault(); ask(); }}>
        <Icon name="sparkle" size={18} />
        <input bind:value={request} placeholder="Describe what you want to hear" aria-label="Ask the DJ" disabled={asking} />
        <button class="pill" type="submit" disabled={!request.trim() || asking}>{asking ? "Picking songs…" : "Ask"}</button>
      </form>
      {#if !answer && !asking && !askError}
        <div class="chips">
          {#each suggestions as sug}<button class="chip" onclick={() => ask(sug)}>{sug}</button>{/each}
        </div>
      {/if}
      {#if asking}<p class="dj-note">The DJ is going through your library…</p>{/if}
      {#if askError}<p class="dj-error">{askError}</p>{/if}
      {#if answer}
        {@const tracks = aiTracks(answer)}
        <div class="answer">
          <div class="answer-head">
            <div>
              <h3>{answer.title}</h3>
              {#if answer.reason}<p>{answer.reason}</p>{/if}
            </div>
            <div class="answer-actions">
              <button class="pill" onclick={() => app.playTracks(tracks)}><Icon name="play" size={15} />Play {tracks.length} songs</button>
              <button class="icon-btn" title="Save as playlist" aria-label="Save as playlist" onclick={() => saveMix(answer!.title, tracks)}><Icon name="playlist" size={17} /></button>
              <button class="icon-btn" title="Close" aria-label="Close" onclick={() => (answer = null)}><Icon name="close" size={17} /></button>
            </div>
          </div>
          <ol class="answer-list">
            {#each tracks.slice(0, 8) as t, i (t.id)}
              <li>
                <button onclick={() => app.playTracks(tracks, i)} oncontextmenu={(e) => app.trackMenu(e, tracks, i)}>
                  <span class="song-art">{#if app.albumById.get(t.albumId)?.artSmall}<img src={artUrl(app.albumById.get(t.albumId)?.artSmall)} alt="" />{/if}</span>
                  <span class="song-text"><span class="song-title">{t.title}</span><span class="song-artist">{t.artist}</span></span>
                </button>
              </li>
            {/each}
          </ol>
          {#if tracks.length > 8}<p class="dj-note">and {tracks.length - 8} more</p>{/if}
          {#if answer.discover?.length}
            <h4 class="discover-title">New to you</h4>
            <ul class="discover">
              {#each answer.discover as d}
                <li>
                  <span class="d-art" class:artist={d.imageIsArtist}>{#if d.image}<img src={d.image} alt="" loading="lazy" />{/if}</span>
                  <span class="d-text"><span class="song-title">{d.title}</span><span class="song-artist">{d.artist}{d.why ? `. ${d.why}` : ""}</span></span>
                  {@render stores(d)}
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
    </section>

    <Carousel title="Made for you today">
      {#snippet actions()}
        <button class="link" onclick={() => app.loadAiMixes(true)} disabled={app.aiMixesState === "loading"}>{app.aiMixesState === "loading" ? "Making mixes…" : "New mixes"}</button>
      {/snippet}
      {#each app.aiMixes as m (m.title)}
        {@render tile(m.title, m.reason, aiTracks(m))}
      {:else}
        {#if app.aiMixesState === "loading"}
          <p class="dj-note">The DJ is making today’s mixes…</p>
        {:else if app.aiMixesState === "error"}
          <p class="dj-error">{app.aiMixesError}</p>
        {/if}
      {/each}
    </Carousel>
  {/if}

  {#if app.aiReady}
    <Carousel title="New music for you">
      {#snippet actions()}
        <button class="link" onclick={() => app.loadAiNew(true)} disabled={app.aiNewState === "loading"}>{app.aiNewState === "loading" ? "Searching…" : "Find more"}</button>
      {/snippet}
      {#each app.aiNew as d (d.artist + d.title)}
        {@render newCard(d)}
      {:else}
        {#if app.aiNewState === "loading"}
          <p class="dj-note">The DJ is searching the web for new songs you’d like…</p>
        {:else if app.aiNewState === "error"}
          <p class="dj-error">{app.aiNewError}</p>
        {/if}
      {/each}
    </Carousel>
  {/if}

  {#if app.mixes.length}
    <Carousel title="Mixes by sound">
      {#each app.mixes as mix (mix.name)}
        {@render tile(mix.name, mix.description, app.mixTracks(mix))}
      {/each}
    </Carousel>
  {:else if app.sonic?.running}
    <p class="listening"><Icon name="wave" size={16} />Cadence is listening to your songs to make mixes by sound ({app.sonic.analyzed} of {app.sonic.total}).</p>
  {/if}

  {#if playedRow.length}
    <Carousel title="Played recently">
      {#each playedRow.slice(0, 30) as album (album.id)}<AlbumCard {album} size={180} />{/each}
    </Carousel>
  {/if}

  {#if app.mostPlayed.length >= 3}
    <section class="row">
      <h2 class="section-title">On repeat</h2>
      <div class="songs">
        {#each app.mostPlayed.slice(0, 9) as t, i (t.id)}
          <button class="song" onclick={() => app.playTracks(app.mostPlayed, i)} oncontextmenu={(e) => app.trackMenu(e, app.mostPlayed, i)}>
            <span class="song-art">{#if app.albumById.get(t.albumId)?.artSmall}<img src={artUrl(app.albumById.get(t.albumId)?.artSmall)} alt="" loading="lazy" />{/if}</span>
            <span class="song-text"><span class="song-title">{t.title}</span><span class="song-artist">{t.artist}</span></span>
            <span class="song-plays">{app.listening?.plays[t.id] ?? 0} {app.listening?.plays[t.id] === 1 ? "play" : "plays"}</span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  {#if addedRow.length}
    <Carousel title="Recently added">
      {#snippet actions()}
        <button class="link" onclick={() => app.go({ name: "recent" })}>See all</button>
      {/snippet}
      {#each addedRow as album (album.id)}<AlbumCard {album} size={180} />{/each}
    </Carousel>
  {/if}

  {#if app.artists.length > 1}
    <Carousel title="Artists">
      {#each app.artists.slice(0, 30) as a (a.name)}
        <button class="artist-chip" onclick={() => app.go({ name: "artist", artist: a.name })}>
          <span class="avatar">{#if a.albums[0]?.artSmall}<img src={artUrl(a.albums[0].artSmall)} alt="" loading="lazy" />{/if}</span>
          <span class="name">{a.name}</span>
        </button>
      {/each}
    </Carousel>
  {/if}

  <p class="stats">
    {app.tracks.length} songs, {app.albums.length} albums, {formatDuration(totalTime)} of music.
    {#if losslessShare === 1}All of it lossless.{:else if losslessShare > 0}{Math.round(losslessShare * 100)}% lossless.{/if}
  </p>
</div>

<style>
  .home {
    padding: 64px 44px 40px;
    max-width: 1500px;
  }
  .hero {
    display: grid;
    grid-template-columns: auto 1fr;
    align-items: end;
    gap: 36px;
    margin-bottom: 48px;
  }
  .cover {
    width: clamp(220px, 24vw, 320px);
    aspect-ratio: 1;
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--surface);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow:
      0 30px 60px rgba(0, 0, 0, 0.5),
      0 10px 60px color-mix(in srgb, var(--accent) 25%, transparent);
    transition: transform 0.3s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .cover:hover {
    transform: scale(1.015);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .info {
    min-width: 0;
    padding-bottom: 6px;
  }
  .state {
    margin: 0 0 10px;
    font-size: 13px;
    font-weight: 600;
    color: var(--accent);
  }
  h1 {
    margin: 0;
    font-family: var(--display);
    font-size: clamp(28px, 3vw, 46px);
    font-weight: 750;
    font-variation-settings: "wdth" 88;
    line-height: 1.02;
    letter-spacing: -0.03em;
    max-width: 22ch;
    overflow-wrap: anywhere;
  }
  .artist {
    margin-top: 10px;
    font-size: 19px;
    color: var(--text-2);
    text-align: left;
  }
  .artist:hover {
    color: var(--text);
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 18px;
    margin-top: 14px;
    font-size: 13px;
    color: var(--text-3);
  }
  .actions {
    display: flex;
    gap: 10px;
    margin-top: 22px;
  }
  .row {
    margin-bottom: 40px;
  }
  .row-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }
  .shelf {
    display: flex;
    gap: 22px;
    overflow-x: auto;
    padding: 16px 4px 12px;
    margin: 0 -4px;
    scrollbar-width: thin;
  }
  .mix {
    width: 210px;
    flex: none;
  }
  .mix-tile {
    position: relative;
    width: 210px;
    height: 210px;
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--surface);
    display: block;
    text-align: left;
    box-shadow: 0 14px 30px rgba(0, 0, 0, 0.4);
    isolation: isolate;
  }
  /* The covers stack like records leaning on each other; the first one sits in front. */
  .layer {
    position: absolute;
    width: 74%;
    aspect-ratio: 1;
    object-fit: cover;
    border-radius: 10px;
    box-shadow: 0 10px 24px rgba(0, 0, 0, 0.5);
    transition: transform 0.35s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .l0 {
    z-index: 3;
    left: 8%;
    top: 8%;
  }
  .l1 {
    z-index: 2;
    left: 22%;
    top: 4%;
    transform: rotate(6deg);
    filter: brightness(0.7);
  }
  .l2 {
    z-index: 1;
    left: 30%;
    top: 2%;
    transform: rotate(11deg);
    filter: brightness(0.5);
  }
  .mix-tile:hover .l1 {
    transform: rotate(9deg) translateX(6px);
  }
  .mix-tile:hover .l2 {
    transform: rotate(15deg) translateX(12px);
  }
  .mix-tile::after {
    content: "";
    position: absolute;
    inset: 0;
    z-index: 4;
    background: linear-gradient(to top, rgba(10, 11, 20, 0.92) 18%, transparent 58%);
  }
  .mix-name {
    position: absolute;
    z-index: 5;
    left: 14px;
    right: 52px;
    bottom: 12px;
    font-family: var(--display);
    font-size: 21px;
    font-weight: 700;
    font-variation-settings: "wdth" 90;
    line-height: 1.05;
    letter-spacing: -0.02em;
    color: var(--text);
  }
  .mix-play {
    position: absolute;
    z-index: 5;
    right: 12px;
    bottom: 12px;
    width: 34px;
    height: 34px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: #0d0e17;
    opacity: 0;
    transform: translateY(4px);
    transition: opacity 0.15s, transform 0.15s;
  }
  .mix-tile:hover .mix-play,
  .mix-tile:focus-visible .mix-play {
    opacity: 1;
    transform: none;
  }
  .mix-desc {
    margin: 9px 2px 0;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .dj {
    margin-bottom: 40px;
  }
  .ask {
    display: flex;
    align-items: center;
    gap: 12px;
    max-width: 720px;
    margin-top: 14px;
    padding: 6px 6px 6px 16px;
    border-radius: 999px;
    background: var(--surface);
    box-shadow: inset 0 0 0 1px var(--line-strong);
    color: var(--accent);
  }
  .ask:focus-within {
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .ask input {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    outline: none;
    font-size: 15px;
    color: var(--text);
    user-select: text;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 12px;
  }
  .chip {
    padding: 6px 13px;
    border-radius: 999px;
    font-size: 13px;
    color: var(--text-2);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .chip:hover {
    color: var(--text);
    background: var(--active);
  }
  .dj-note,
  .dj-error {
    margin: 12px 0 0;
    font-size: 13px;
    color: var(--text-3);
  }
  .dj-error {
    color: var(--warn);
    max-width: 70ch;
  }
  .answer {
    max-width: 900px;
    margin-top: 16px;
    padding: 18px 18px 14px;
    border-radius: var(--r-lg);
    background: color-mix(in srgb, var(--surface) 92%, var(--accent));
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .answer-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 20px;
  }
  .answer h3 {
    margin: 0;
    font-family: var(--display);
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }
  .answer-head p {
    margin: 4px 0 0;
    color: var(--text-2);
    font-size: 13.5px;
  }
  .answer-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .answer-list {
    list-style: none;
    margin: 14px 0 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 2px 16px;
  }
  .answer-list button {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 8px 5px 5px;
    border-radius: var(--r-md);
    text-align: left;
    min-width: 0;
  }
  .answer-list button:hover {
    background: var(--hover);
  }
  .answer-list .song-art {
    width: 36px;
    height: 36px;
  }
  .discover-title {
    margin: 18px 0 6px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-2);
  }
  .discover {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .new-card {
    width: 180px;
  }
  .new-art {
    width: 180px;
    height: 180px;
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--surface);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
  }
  /* An artist photo (no cover found) is shown round, like on the Artists page. */
  .new-art.artist {
    border-radius: 50%;
  }
  .new-art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .new-title,
  .new-artist {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .new-title {
    margin-top: 10px;
    font-weight: 600;
    font-size: 13.5px;
  }
  .new-artist {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .new-why {
    margin: 6px 0 0;
    font-size: 12px;
    line-height: 1.4;
    color: var(--text-3);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .stores {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-top: 8px;
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
  .discover .stores {
    margin-top: 0;
    flex: none;
  }
  .d-art {
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--surface);
  }
  .d-art.artist {
    border-radius: 50%;
  }
  .d-art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .discover li {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 6px 8px;
    border-radius: var(--r-md);
  }
  .discover li:hover {
    background: var(--hover);
  }
  .d-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .small {
    font-size: 12.5px;
    flex: none;
  }
  .listening {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: -20px 0 36px;
    font-size: 13px;
    color: var(--text-3);
  }
  .songs {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 4px 20px;
    margin-top: 14px;
  }
  .song {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 7px 10px 7px 7px;
    border-radius: var(--r-md);
    text-align: left;
    min-width: 0;
  }
  .song:hover {
    background: var(--hover);
  }
  .song-art {
    width: 46px;
    height: 46px;
    flex: none;
    border-radius: 7px;
    overflow: hidden;
    background: var(--surface);
  }
  .song-art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .song-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .song-title,
  .song-artist {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .song-title {
    font-weight: 500;
  }
  .song-artist {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .song-plays {
    font-size: 12px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .artist-chip {
    width: 128px;
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .avatar {
    width: 128px;
    height: 128px;
    border-radius: 50%;
    overflow: hidden;
    background: var(--surface);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
    transition: transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .artist-chip:hover .avatar {
    transform: scale(1.04);
  }
  .avatar img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .name {
    font-weight: 600;
    font-size: 13px;
    max-width: 100%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .stats {
    color: var(--text-3);
    font-size: 13px;
    margin: 8px 0 0;
  }
</style>

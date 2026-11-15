<script lang="ts">
  import { api, artUrl, formatDuration, type Album } from "#lib/api.js";
  import AlbumCard from "#lib/components/AlbumCard.svelte";
  import Icon from "#lib/components/Icon.svelte";
  import Segmented from "#lib/components/Segmented.svelte";
  import TrackList from "#lib/components/TrackList.svelte";
  import { app } from "#lib/store.svelte.js";

  let { name }: { name: string } = $props();
  // Under any of its spellings (米津玄師 opens Kenshi Yonezu's page when they're one artist).
  const artist = $derived(app.artistByName(name));
  const also = $derived(artist?.also ?? []);

  /** "Not the same artist": `other` is split out of this artist (no file changes; Undo puts it back). */
  async function notSame(other: string) {
    if (!artist) return;
    const shown = artist.name;
    const rest = [shown, ...also].filter((n) => n !== other);
    try {
      const pairs = await api.artistMarkDifferent(other, rest);
      app.showToast(`“${other}” is shown as another artist now`, {
        label: "Undo",
        run: () => void api.artistForget(pairs).catch((e) => app.showToast(String(e))),
      });
    } catch (e) {
      app.showToast(String(e));
    }
  }
  // How the albums are ordered, remembered for every artist.
  type Order = "newest" | "oldest" | "az";
  const ORDER_KEY = "cadence-artist-album-order";
  const orders: { value: Order; label: string }[] = [
    { value: "newest", label: "Newest first" },
    { value: "oldest", label: "Oldest first" },
    { value: "az", label: "A–Z" },
  ];
  function savedOrder(): Order {
    try {
      const v = localStorage.getItem(ORDER_KEY);
      return v === "oldest" || v === "az" ? v : "newest";
    } catch {
      return "newest";
    }
  }
  let order = $state<Order>(savedOrder());
  function setOrder(o: Order) {
    order = o;
    try {
      localStorage.setItem(ORDER_KEY, o);
    } catch {
      // The order still applies until the page is closed.
    }
  }
  const byTitle = (a: Album, b: Album) => a.title.localeCompare(b.title);
  const sorters: Record<Order, (a: Album, b: Album) => number> = {
    newest: (a, b) => (b.year ?? 0) - (a.year ?? 0) || byTitle(a, b),
    // Albums without a year go last, not first.
    oldest: (a, b) => (a.year || Infinity) - (b.year || Infinity) || byTitle(a, b),
    az: byTitle,
  };
  const albums = $derived([...(artist?.albums ?? [])].sort(sorters[order]));
  const songs = $derived(albums.flatMap((a) => app.albumTracks(a)));
  // The picture's glow comes from the newest album, whatever the order.
  const portrait = $derived([...(artist?.albums ?? [])].sort(sorters.newest)[0]);
  const picture = $derived(artist ? app.artistPicture(artist, "large") : null);
  const time = $derived(songs.reduce((s, t) => s + t.duration, 0));
</script>

{#if artist}
  <div class="artist-page">
    <header class="hero">
      <button class="portrait" style="--glow: {portrait?.accent ?? 'transparent'}" onclick={() => (app.artistEditing = artist.name)} aria-label="Change the picture for {artist.name}" title="Change picture">
        {#key picture}
          {#if picture}<img src={artUrl(picture)} alt="" />{:else}<Icon name="artist" size={64} />{/if}
        {/key}
        <span class="edit"><Icon name="edit" size={20} /><span>Change</span></span>
      </button>
      <div class="info">
        <h1>{artist.name}</h1>
        {#if also.length}
          <p class="also">
            Also written as
            {#each also as other, i (other)}
              {i ? ", " : ""}<span class="spelling">{other}</span>
              <button class="link small" onclick={() => notSame(other)} title="“{other}” is someone else: show them as another artist">Not the same artist</button>
            {/each}
          </p>
        {/if}
        <p class="facts">{albums.length} {albums.length === 1 ? "album" : "albums"}, {songs.length} {songs.length === 1 ? "song" : "songs"}, {formatDuration(time)}</p>
        <div class="actions">
          <button class="pill" onclick={() => app.playTracks(songs)}><Icon name="play" size={15} />Play</button>
          <button class="pill ghost" onclick={() => app.shuffleTracks(songs)}><Icon name="shuffle" size={15} />Shuffle</button>
          {#if albums.length > 1}
            <button class="pill ghost" onclick={() => app.shuffleAlbums(albums)} title="A random album of {artist.name}’s, whole and in order, then another">
              <Icon name="album" size={15} />Shuffle albums
            </button>
            <button class="pill ghost" onclick={() => app.surpriseAlbum(albums)} title="Play one of {artist.name}’s albums, picked at random">
              <Icon name="sparkle" size={15} />Surprise me
            </button>
          {/if}
        </div>
      </div>
    </header>

    <section>
      <div class="albums-head">
        <h2 class="section-title">Albums</h2>
        {#if albums.length > 1}<Segmented label="Order of albums" options={orders} value={order} onchange={setOrder} />{/if}
      </div>
      <div class="grid">
        {#each albums as album (album.id)}<AlbumCard {album} />{/each}
      </div>
    </section>

    <section>
      <h2 class="section-title">Songs</h2>
      <TrackList tracks={songs} showArt showAlbum />
    </section>
  </div>
{:else}
  <div class="page secondary">No music by {name} in your library.</div>
{/if}

<style>
  .artist-page {
    padding: 64px 44px 40px;
  }
  .hero {
    display: flex;
    align-items: center;
    gap: 32px;
    margin-bottom: 40px;
  }
  .portrait {
    width: 190px;
    height: 190px;
    flex: none;
    border-radius: 50%;
    overflow: hidden;
    background: var(--surface);
    display: grid;
    place-items: center;
    color: var(--text-3);
    box-shadow:
      0 20px 50px rgba(0, 0, 0, 0.5),
      0 6px 40px color-mix(in srgb, var(--glow) 30%, transparent);
  }
  .portrait {
    position: relative;
    padding: 0;
  }
  .portrait img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    animation: picture-in 0.4s ease-out;
  }
  @keyframes picture-in {
    from {
      opacity: 0;
    }
  }
  /* "Change" appears over the picture on hover (always reachable by keyboard). */
  .edit {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    font-size: 13px;
    font-weight: 600;
    color: white;
    background: rgba(0, 0, 0, 0.5);
    opacity: 0;
    transition: opacity 0.18s;
  }
  .portrait:hover .edit,
  .portrait:focus-visible .edit {
    opacity: 1;
  }
  @media (hover: none) {
    .edit {
      inset: auto 0 0;
      height: 34%;
      flex-direction: row;
      opacity: 1;
      background: linear-gradient(transparent, rgba(0, 0, 0, 0.6));
    }
    .edit span {
      display: none;
    }
  }
  h1 {
    margin: 0;
    font-family: var(--display);
    font-size: clamp(32px, 4vw, 56px);
    font-weight: 750;
    font-variation-settings: "wdth" 88;
    letter-spacing: -0.03em;
    line-height: 1;
  }
  .also {
    margin: 8px 0 0;
    color: var(--text-2);
    font-size: 13px;
  }
  .also .spelling {
    color: var(--text-1, inherit);
  }
  .also .link {
    margin-left: 4px;
    font-size: 12px;
    color: var(--text-3);
    text-decoration: underline;
  }
  .facts {
    margin: 10px 0 0;
    color: var(--text-2);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 20px;
  }
  section {
    margin-top: 38px;
  }
  section .section-title {
    margin-bottom: 18px;
  }
  .albums-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 18px;
  }
  .albums-head .section-title {
    margin-bottom: 0;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 30px 24px;
  }
  /* Phones: cover on top, text below. */
  @media (max-width: 720px) {
    .hero {
      display: flex !important;
      flex-direction: column;
      align-items: flex-start !important;
      gap: 18px !important;
      margin-bottom: 28px;
    }
    .hero .portrait {
      width: min(56vw, 220px) !important;
      height: auto !important;
      aspect-ratio: 1;
      align-self: center;
    }
    h1 {
      font-size: 30px !important;
      max-width: none !important;
    }
    .actions {
      flex-wrap: wrap;
    }
  }
  /* Phones: two albums per row. */
  @media (max-width: 720px) {
    .grid {
      grid-template-columns: repeat(2, minmax(0, 1fr)) !important;
      gap: 20px 14px !important;
    }
  }
</style>

<script lang="ts">
  // Choosing an artist's picture: a photo from the online catalogue, one of their album covers,
  // or an image file. Picking one saves it straight away.
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, artUrl, type ArtistPhoto } from "$lib/api";
  import { app } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  const name = $derived(app.artistEditing!);
  const artist = $derived(app.artists.find((a) => a.name.toLowerCase() === name.toLowerCase()));
  const covers = $derived((artist?.albums ?? []).filter((a) => a.art));
  const custom = $derived(!!app.artistImages[name.toLowerCase()]);
  const isAndroid = navigator.userAgent.includes("Android");

  let photos = $state<ArtistPhoto[]>([]);
  let searching = $state(true);
  let saving = $state<string | null>(null);
  let error = $state("");

  api
    .findArtistPhotos(app.artistEditing!)
    .then((p) => (photos = p))
    .catch((e) => (error = String(e)))
    .finally(() => (searching = false));

  async function use(source: { kind: "url"; url: string } | { kind: "file"; path: string }, key: string) {
    saving = key;
    error = "";
    try {
      await app.setArtistImage(name, source);
      close();
    } catch (e) {
      error = String(e);
    } finally {
      saving = null;
    }
  }

  async function chooseFile() {
    const path = await open({ title: `Picture for ${name}`, filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "webp", "gif", "bmp"] }] });
    if (typeof path === "string") await use({ kind: "file", path }, "file");
  }

  async function reset() {
    await app.resetArtistImage(name);
    close();
  }

  const close = () => (app.artistEditing = null);
</script>

<Modal title="Picture for {name}" onclose={close} width="640px">
  <h3>From the internet</h3>
  <div class="row">
    {#if searching}
      {#each [0, 1, 2, 3] as i (i)}<div class="opt ghost-opt"><span class="pic"></span><i></i></div>{/each}
    {:else if photos.length}
      {#each photos as p, i (i)}
        <button class="opt" class:busy={saving === p.picture} disabled={!!saving} onclick={() => use({ kind: "url", url: p.picture }, p.picture)} title="Use this photo of {p.name}">
          <span class="pic"><img src={p.thumb} alt="" /></span>
          <span class="label">{p.name}</span>
        </button>
      {/each}
    {:else if !error}
      <p class="secondary none">No photos of “{name}” found online.</p>
    {/if}
  </div>

  {#if covers.length}
    <h3>From your albums</h3>
    <div class="row">
      {#each covers as a (a.id)}
        <button class="opt" class:busy={saving === a.id} disabled={!!saving} onclick={() => use({ kind: "file", path: a.art! }, a.id)} title="Use the cover of {a.title}">
          <span class="pic"><img src={artUrl(a.artSmall ?? a.art)} alt="" loading="lazy" /></span>
          <span class="label">{a.title}</span>
        </button>
      {/each}
    </div>
  {/if}

  {#if error}<p class="error">{error}</p>{/if}

  {#snippet footer()}
    {#if custom}<button class="link note" onclick={reset}>Use the album cover again</button>{:else}<span class="note"></span>{/if}
    {#if !isAndroid}<button class="pill ghost" onclick={chooseFile} disabled={!!saving}><Icon name="folder" size={15} />Choose a file…</button>{/if}
    <button class="pill ghost" onclick={close}>Cancel</button>
  {/snippet}
</Modal>

<style>
  h3 {
    margin: 0 0 12px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-2);
  }
  h3:not(:first-child) {
    margin-top: 22px;
  }
  .row {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
    gap: 14px 12px;
  }
  .opt {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    min-width: 0;
    padding: 6px 4px;
    border-radius: var(--r-sm);
  }
  .opt:hover:not(:disabled) .pic {
    transform: scale(1.04);
    box-shadow: 0 0 0 3px var(--accent);
  }
  .opt:disabled {
    cursor: default;
  }
  .opt:disabled:not(.busy) {
    opacity: 0.5;
  }
  .busy .pic {
    box-shadow: 0 0 0 3px var(--accent);
    animation: pulse 1s ease-in-out infinite;
  }
  .pic {
    width: 88px;
    height: 88px;
    border-radius: 50%;
    overflow: hidden;
    background: var(--surface);
    transition:
      transform 0.2s cubic-bezier(0.2, 0.8, 0.2, 1),
      box-shadow 0.2s;
  }
  .pic img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .label {
    max-width: 100%;
    font-size: 12.5px;
    color: var(--text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ghost-opt {
    pointer-events: none;
  }
  .ghost-opt .pic,
  .ghost-opt i {
    animation: pulse 1.2s ease-in-out infinite;
  }
  .ghost-opt i {
    display: block;
    width: 60px;
    height: 9px;
    border-radius: 5px;
    background: var(--surface);
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
  .none {
    grid-column: 1 / -1;
    margin: 0;
    font-size: 13px;
  }
  .error {
    margin: 14px 0 0;
    color: var(--warn);
    font-size: 13px;
  }
  .note {
    margin-right: auto;
    align-self: center;
    font-size: 13px;
  }
</style>

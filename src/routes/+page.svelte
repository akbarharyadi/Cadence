<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { api, artUrl, khz, qualityOf } from "$lib/api";
  import { app } from "$lib/store.svelte";
  import ContextMenu from "$lib/components/ContextMenu.svelte";
  import FolderBrowser from "$lib/components/FolderBrowser.svelte";
  import UpdateBanner from "$lib/components/UpdateBanner.svelte";
  import { updates } from "$lib/updates.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import NowPlaying from "$lib/components/NowPlaying.svelte";
  import PlaylistPicker from "$lib/components/PlaylistPicker.svelte";
  import SmartEditor from "$lib/components/SmartEditor.svelte";
  import TagEditor from "$lib/components/TagEditor.svelte";
  import FixInfo from "$lib/components/FixInfo.svelte";
  import ArtistPicture from "$lib/components/ArtistPicture.svelte";
  import PlayerBar from "$lib/components/PlayerBar.svelte";
  import QueuePanel from "$lib/components/QueuePanel.svelte";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import TabBar from "$lib/components/TabBar.svelte";
  import TitleBar from "$lib/components/TitleBar.svelte";
  import AlbumView from "$lib/views/AlbumView.svelte";
  import Albums from "$lib/views/Albums.svelte";
  import ArtistView from "$lib/views/ArtistView.svelte";
  import Artists from "$lib/views/Artists.svelte";
  import Home from "$lib/views/Home.svelte";
  import PlaylistView from "$lib/views/PlaylistView.svelte";
  import Search from "$lib/views/Search.svelte";
  import Settings from "$lib/views/Settings.svelte";
  import Songs from "$lib/views/Songs.svelte";
  import SimilarView from "$lib/views/SimilarView.svelte";
  import Sound from "$lib/views/Sound.svelte";
  import Welcome from "$lib/views/Welcome.svelte";

  const DEFAULT_ACCENT = "#8b7cff";
  const DEFAULT_TINT = "#1b1c33";

  let dropping = $state(false);
  let scrolled = $state(false);
  // Once the page's big title scrolls under the top bar, it shows small in the bar (as on iOS).
  let barTitle = $state("");
  let showBarTitle = $state(false);
  function onScroll(e: Event) {
    const el = e.currentTarget as HTMLElement;
    scrolled = el.scrollTop > 4;
    const h1 = el.querySelector("h1");
    if (!h1) {
      showBarTitle = false;
      return;
    }
    barTitle = h1.textContent?.trim() ?? "";
    showBarTitle = h1.getBoundingClientRect().bottom < el.getBoundingClientRect().top + 56;
  }
  // A new page starts with its own title hidden.
  $effect(() => {
    void app.view;
    showBarTitle = false;
  });

  // Android: report what's playing to the media notification, lock screen and widget, and take
  // their button presses. Only sent when the song or play/pause changes, or every 15 s.
  const android = typeof window !== "undefined" ? (window as unknown as { CadenceAndroid?: { update(json: string): void } }).CadenceAndroid : undefined;
  let lastAndroidKey = "";
  let lastAndroidAt = 0;
  $effect(() => {
    if (!android) return;
    const t = app.current;
    const s = app.status;
    const playing = !!s?.playing;
    const key = `${t?.id ?? ""}|${playing}|${s?.source?.sampleRate ?? 0}`;
    const now = Date.now();
    if (key === lastAndroidKey && now - lastAndroidAt < 15000) return;
    lastAndroidKey = key;
    lastAndroidAt = now;
    android.update(
      JSON.stringify({
        title: t?.title ?? "",
        artist: t?.artist ?? "",
        album: t?.album ?? "",
        art: app.currentAlbum?.art ?? "",
        playing,
        position: s?.position ?? 0,
        duration: s?.duration ?? 0,
        quality: s?.source ? `${qualityOf(s.source.codec, s.source.sampleRate)} ${s.source.bits ? `${s.source.bits}-bit/` : ""}${khz(s.source.sampleRate)}` : "",
        accent: app.currentAlbum?.accent ?? "",
      }),
    );
  });

  onMount(() => {
    app.init();
    updates.start();
    (window as unknown as { __cadenceBack: () => boolean }).__cadenceBack = () => app.handleBack();
    (window as unknown as { __cadenceMedia: (c: string) => void }).__cadenceMedia = (c: string) => {
      if (c.startsWith("seek:")) app.seek(Number(c.slice(5)));
      else if (c === "play" || c === "pause" || c === "next" || c === "prev") api.player(c);
    };
    // On a phone the first scan can run before the user has allowed access to their music; look
    // again when Cadence comes back to the foreground while the library is still empty.
    const retry = () => {
      if (app.loaded && app.tracks.length === 0 && app.folders.length > 0 && !app.scan) api.rescan();
    };
    window.addEventListener("focus", retry);
    document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && retry());
    // Drop folders or files anywhere on the window to import them.
    const unlisten = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "enter" || e.payload.type === "over") dropping = true;
      else if (e.payload.type === "leave") dropping = false;
      else if (e.payload.type === "drop") {
        dropping = false;
        app.addFolders(e.payload.paths);
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  });

  // The whole app takes its colour from the album that's playing (or open).
  $effect(() => {
    const root = document.documentElement.style;
    root.setProperty("--accent", app.themeAlbum?.accent ?? DEFAULT_ACCENT);
    root.setProperty("--tint", app.themeAlbum?.tint ?? DEFAULT_TINT);
  });

  function onKey(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const typing = (target.tagName === "INPUT" && (target as HTMLInputElement).type !== "range") || target.tagName === "TEXTAREA";
    if (typing) return;
    // Space on a focused button already clicks it; don't also toggle playback.
    const pressable = target.tagName === "BUTTON" || target.getAttribute("role") === "option";
    if (e.key === " " && !pressable) {
      e.preventDefault();
      api.player("toggle");
    } else if (e.ctrlKey && e.key === "ArrowRight") {
      api.player("next");
    } else if (e.ctrlKey && e.key === "ArrowLeft") {
      api.player("prev");
    } else if (!e.ctrlKey && !e.altKey && (e.key === "ArrowRight" || e.key === "ArrowLeft") && app.current && target.tagName !== "INPUT") {
      app.seek(Math.max(0, app.position + (e.key === "ArrowRight" ? 10 : -10)));
    } else if (e.altKey && e.key === "ArrowLeft") {
      app.back();
    }
  }

  const empty = $derived(app.loaded && app.tracks.length === 0);
  const showSidebar = $derived(!empty || app.folders.length > 0);
  // Phones (and very narrow windows) get a bottom tab bar instead of the sidebar.
  let phone = $state(false);
  $effect(() => {
    // Android always gets the phone layout, whatever the width (and in landscape).
    const android = navigator.userAgent.includes("Android");
    if (android) document.documentElement.classList.add("phone");
    const mq = matchMedia("(max-width: 720px)");
    phone = android || mq.matches;
    const on = () => (phone = android || mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  });
  const ambient = $derived(artUrl(app.themeAlbum?.artSmall));
</script>

<svelte:window onkeydown={onKey} />
<!-- No browser menu (Back, Refresh, Print…) on right-click; text fields keep theirs for copy and paste. -->
<svelte:document
  onmouseup={(e) => e.button === 3 && app.back()}
  oncontextmenu={(e) => !(e.target as HTMLElement).closest("input, textarea, [contenteditable]") && e.preventDefault()}
/>

<!-- Ambient colour field from the current cover, behind everything. -->
<div class="ambient" aria-hidden="true">
  {#key ambient}
    {#if ambient}<div class="cover" style="background-image: url('{ambient}')"></div>{/if}
  {/key}
</div>

<TitleBar />

<div class="shell" class:docked={!!app.current}>
  <div class="body">
    {#if showSidebar && !phone}<Sidebar />{/if}
    <div class="column">
    <main class="content" onscroll={onScroll}>
      <!-- Sticky top bar: Back button, and a place to drag the frameless window from. -->
      <div class="topbar" class:scrolled data-tauri-drag-region>
        {#if app.history.length > 0 && (!empty || app.view.name === "sound" || app.view.name === "settings")}
          <button class="back icon-btn" onclick={() => app.back()} aria-label="Back"><Icon name="back" size={18} /></button>
        {/if}
        <div class="bar-title" class:on={showBarTitle} aria-hidden={!showBarTitle}>{barTitle}</div>
      </div>
      {#if !app.loaded}
        <div></div>
      {:else if app.view.name === "sound"}
        <Sound />
      {:else if app.view.name === "settings"}
        <Settings />
      {:else if empty}
        <Welcome />
      {:else if app.view.name === "home"}
        <Home />
      {:else if app.view.name === "albums"}
        <Albums />
      {:else if app.view.name === "recent"}
        <Albums recent />
      {:else if app.view.name === "album"}
        {#key app.view.id}<AlbumView id={app.view.id} />{/key}
      {:else if app.view.name === "artists"}
        <Artists />
      {:else if app.view.name === "artist"}
        {#key app.view.artist}<ArtistView name={app.view.artist} />{/key}
      {:else if app.view.name === "songs"}
        <Songs />
      {:else if app.view.name === "search"}
        <Search query={app.view.query} />
      {:else if app.view.name === "playlist"}
        {#key app.view.id}<PlaylistView id={app.view.id} />{/key}
      {:else if app.view.name === "similar"}
        {#key app.view.id}<SimilarView id={app.view.id} />{/key}
      {/if}
    </main>
    </div>
    {#if app.queueOpen}<QueuePanel />{/if}
  </div>
  <PlayerBar />
  {#if phone && showSidebar}<TabBar />{/if}
</div>

{#if app.nowPlayingOpen}<NowPlaying />{/if}
<ContextMenu />
{#if app.picker}<PlaylistPicker />{/if}
{#if app.smartEditing}<SmartEditor />{/if}
{#if app.tagEditing}<TagEditor />{/if}
{#if app.fixing}{#key app.fixing.id}<FixInfo />{/key}{/if}
{#if app.artistEditing}{#key app.artistEditing}<ArtistPicture />{/key}{/if}
{#if app.folderBrowser}<FolderBrowser />{/if}
<UpdateBanner />

{#if dropping}
  <div class="dropzone">
    <div><Icon name="folder" size={44} /><p>Drop to add to your library</p></div>
  </div>
{/if}

{#if app.toast}
  <div class="toast" role="status">{app.toast}</div>
{/if}

<style>
  .ambient {
    position: fixed;
    inset: 0;
    z-index: 0;
    pointer-events: none;
    background:
      radial-gradient(ellipse 90% 60% at 30% -10%, color-mix(in srgb, var(--tint) 85%, transparent), transparent 70%),
      var(--ink);
    overflow: hidden;
  }
  .ambient .cover {
    position: absolute;
    inset: -20% -10% 45% -10%;
    background-size: cover;
    background-position: center 30%;
    filter: blur(110px) saturate(1.5);
    opacity: 0.32;
    mask-image: linear-gradient(to bottom, black 20%, transparent 95%);
    animation: fadein 1.2s ease;
  }
  @keyframes fadein {
    from {
      opacity: 0;
    }
  }
  .shell {
    position: relative;
    z-index: 1;
    height: 100vh;
    display: flex;
    flex-direction: column;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .column {
    flex: 1;
    min-width: 0;
    position: relative;
    display: flex;
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    position: relative;
    padding-bottom: 32px;
    transition: padding-bottom 0.3s;
  }
  /* Leave room for the floating player once something is queued. */
  .docked .content {
    padding-bottom: 104px;
  }
  .topbar {
    position: sticky;
    top: 0;
    z-index: 20;
    height: 52px;
    margin-bottom: -52px;
    /* Above the backdrop, so Back stays clickable. */
    z-index: 22;
    display: flex;
    align-items: center;
    padding: 0 20px;
    transition: background 0.2s, box-shadow 0.2s;
  }
  /* Scrolled: the bar frosts over the page (it lives inside the page, so the Back button and the
     title stay on top of it during page animations). */
  .topbar.scrolled {
    background: color-mix(in srgb, var(--ink) 78%, transparent);
    backdrop-filter: blur(22px) saturate(1.4);
    box-shadow: 0 1px 0 var(--line);
  }
  .back {
    background: var(--hover);
  }
  .bar-title {
    position: absolute;
    left: 64px;
    right: 64px;
    text-align: center;
    font-family: var(--display);
    font-weight: 700;
    font-size: 16px;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    pointer-events: none;
    opacity: 0;
    transform: translateY(6px);
    transition:
      opacity 0.2s,
      transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .bar-title.on {
    opacity: 1;
    transform: none;
  }
  /* Desktop: clear of the window buttons in the corner. */
  @media (min-width: 721px) {
    .bar-title {
      left: 170px;
      right: 170px;
    }
  }
  .dropzone {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--ink) 75%, transparent);
    backdrop-filter: blur(8px);
    pointer-events: none;
  }
  .dropzone div {
    text-align: center;
    padding: 50px 70px;
    border-radius: var(--r-lg);
    border: 2px dashed var(--accent);
    color: var(--accent);
  }
  .dropzone p {
    color: var(--text);
    font-family: var(--display);
    font-size: 20px;
    font-weight: 600;
    margin: 14px 0 0;
  }
  .toast {
    position: fixed;
    bottom: 120px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 95;
    padding: 10px 18px;
    border-radius: 999px;
    background: rgba(36, 39, 61, 0.95);
    backdrop-filter: blur(20px);
    box-shadow:
      0 0 0 1px var(--line-strong),
      0 10px 30px rgba(0, 0, 0, 0.45);
    max-width: 70vw;
    animation: up 0.2s ease-out;
  }
  @keyframes up {
    from {
      opacity: 0;
      transform: translate(-50%, 10px);
    }
  }
</style>

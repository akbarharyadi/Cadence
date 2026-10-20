<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, khz } from "$lib/api";
  import { onMount } from "svelte";
  import AiSettings from "$lib/components/AiSettings.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Toggle from "$lib/components/Toggle.svelte";
  import { app } from "$lib/store.svelte";

  let closeToTray = $state(true);
  let lyricsOnline = $state(true);
  let lbUser = $state("");
  let lbToken = $state("");
  let lbBusy = $state(false);
  let lbError = $state("");
  onMount(async () => {
    const s = await api.appSettings();
    closeToTray = s.closeToTray;
    lyricsOnline = s.lyricsOnline;
    lbUser = s.listenbrainzUser;
  });

  async function connectLb() {
    lbBusy = true;
    lbError = "";
    try {
      lbUser = await api.connectListenBrainz(lbToken.trim());
      lbToken = "";
    } catch (e) {
      lbError = String(e);
    } finally {
      lbBusy = false;
    }
  }

  async function disconnectLb() {
    await api.disconnectListenBrainz();
    lbUser = "";
  }

  const out = $derived(app.status?.output);
  const src = $derived(app.status?.source);

  async function addFolder() {
    const picked = await open({ directory: true, multiple: true, title: "Add music folders" });
    if (picked) await app.addFolders(Array.isArray(picked) ? picked : [picked]);
  }

  async function remove(path: string) {
    app.folders = await api.removeFolder(path);
  }

  const shortcuts: [string[], string][] = [
    [["Space"], "Play or pause"],
    [["Ctrl", "→"], "Next song"],
    [["Ctrl", "←"], "Previous song"],
    [["→"], "Forward 10 seconds"],
    [["←"], "Back 10 seconds"],
    [["Ctrl", "K"], "Search"],
    [["Alt", "←"], "Go back"],
    [["Esc"], "Close Now Playing"],
    [["Media keys"], "Play, pause, next and previous, even when Cadence is in the background"],
  ];
</script>

<div class="page">
  <h1 class="page-title">Settings</h1>

  <section>
    <div class="title-row">
      <div>
        <h2 class="section-title">Music folders</h2>
        <p class="desc">Cadence finds FLAC, ALAC, WAV, AIFF, MP3, AAC and Ogg files in these folders. You can also drop folders onto the window.</p>
      </div>
      <button class="pill" onclick={addFolder}><Icon name="plus" size={15} />Add folder</button>
    </div>
    <ul class="folders">
      {#each app.folders as f (f)}
        <li>
          <Icon name="folder" size={18} />
          <span class="path" title={f}>{f}</span>
          <button class="icon-btn" aria-label="Remove {f}" title="Remove from Cadence (your files stay where they are)" onclick={() => remove(f)}>
            <Icon name="trash" size={16} />
          </button>
        </li>
      {:else}
        <li class="none">No folders yet. Add one to start your library.</li>
      {/each}
    </ul>
    <div class="row">
      <span class="secondary small">{app.tracks.length} songs in {app.albums.length} albums</span>
      <button class="link small" onclick={() => api.rescan()} disabled={!!app.scan}>{app.scan ? "Looking for changes…" : "Look for changes now"}</button>
    </div>
  </section>

  <section>
    <h2 class="section-title">Sound analysis</h2>
    <p class="desc">Cadence listens to each song once (tempo, timbre, loudness and harmony) to make mixes by sound, sound radio and “sounds like” lists. It all happens on this computer.</p>
    {#if app.sonic}
      {@const s = app.sonic}
      <div class="meter" role="progressbar" aria-valuemin={0} aria-valuemax={s.total} aria-valuenow={s.analyzed} aria-label="Songs analysed">
        <span style="width: {s.total ? (s.analyzed / s.total) * 100 : 0}%"></span>
      </div>
      <p class="hint">
        {#if s.running}Listening… {s.analyzed} of {s.total} songs{:else if s.analyzed >= s.total}All {s.total} songs analysed{:else}{s.analyzed} of {s.total} songs analysed{/if}{app.mixes.length ? `, ${app.mixes.length} ${app.mixes.length === 1 ? "mix" : "mixes"} on Home` : ""}.
      </p>
    {/if}
  </section>

  <section>
    <AiSettings />
  </section>

  <section>
    <div class="title-row">
      <div>
        <h2 class="section-title">Audio output</h2>
        <p class="desc">Cadence plays through your Windows default device.</p>
      </div>
      <button class="pill ghost" onclick={() => app.go({ name: "sound" })}><Icon name="eq" size={15} />Sound settings</button>
    </div>
    {#if out?.device}
      <dl class="facts">
        <dt>Device</dt>
        <dd>{out.device}</dd>
        <dt>Device format</dt>
        <dd>{khz(out.sampleRate)}, {out.channels} channels, through the Windows mixer</dd>
        {#if src}
          <dt>Playing</dt>
          <dd>{src.codec} {src.bits ? `${src.bits}-bit/` : ""}{khz(src.sampleRate)}</dd>
        {/if}
      </dl>
      {#if out.resampling && src}
        <div class="tip">
          <Icon name="wave" size={18} />
          <p>
            Your device runs at {khz(out.sampleRate)}, so this {khz(src.sampleRate)} music is converted on the way (with a high-quality resampler).
            To play it at its own rate, open Windows Sound settings, choose {out.device}, and set its format to <b>{khz(src.sampleRate)}</b>.
          </p>
        </div>
      {/if}
    {:else}
      <p class="desc">{app.status?.error ?? "No audio device found. Connect speakers or headphones and Cadence will pick them up."}</p>
    {/if}
  </section>

  <section>
    <div class="title-row">
      <div>
        <h2 class="section-title">Keep playing in the tray</h2>
        <p class="desc">When you close the window, the music keeps playing and Cadence stays in the system tray. Click the tray icon to bring it back; quit from its menu.</p>
      </div>
      <Toggle checked={closeToTray} label="Keep playing in the tray" onchange={(v) => { closeToTray = v; api.setCloseToTray(v); }} />
    </div>
  </section>

  <section>
    <div class="title-row">
      <div>
        <h2 class="section-title">Find lyrics online</h2>
        <p class="desc">When a song has no lyrics in the file or a .lrc next to it, look them up on LRCLIB, a free lyrics database. Only the artist, title and album are sent.</p>
      </div>
      <Toggle checked={lyricsOnline} label="Find lyrics online" onchange={(v) => { lyricsOnline = v; api.setLyricsOnline(v); }} />
    </div>
  </section>

  <section>
    <h2 class="section-title">Scrobbling</h2>
    <p class="desc">Keep a record of what you listen to on ListenBrainz, a free and open listening history. A song counts once you’ve heard half of it or four minutes.</p>
    {#if lbUser}
      <div class="connected">
        <span class="dot"></span>
        <span>Sending your listens to ListenBrainz as <b>{lbUser}</b></span>
        <button class="pill ghost" onclick={disconnectLb}>Disconnect</button>
      </div>
    {:else}
      <form class="connect" onsubmit={(e) => { e.preventDefault(); connectLb(); }}>
        <input class="field" type="password" bind:value={lbToken} placeholder="Paste your ListenBrainz user token" aria-label="ListenBrainz user token" autocomplete="off" />
        <button class="pill" type="submit" disabled={!lbToken.trim() || lbBusy}>{lbBusy ? "Checking…" : "Connect"}</button>
      </form>
      <p class="hint">Find your token at listenbrainz.org, under your account settings. It’s stored in Windows Credential Manager, not in Cadence’s files.</p>
      {#if lbError}<p class="error">{lbError}</p>{/if}
    {/if}
  </section>

  <section>
    <h2 class="section-title">Keyboard shortcuts</h2>
    <dl class="keys">
      {#each shortcuts as [keys, action]}
        <dt>{#each keys as k}<kbd>{k}</kbd>{/each}</dt>
        <dd>{action}</dd>
      {/each}
    </dl>
  </section>

  <section>
    <h2 class="section-title">About</h2>
    <p class="desc">
      Cadence 0.1, a free, open-source lossless music player for your own files. Rust audio engine with ReplayGain 2.0 leveling,
      a 64-bit parametric equalizer, crossfeed and high-quality resampling. Not for commercial use.
    </p>
  </section>
</div>

<style>
  .page {
    max-width: 1000px;
  }
  section {
    margin-top: 32px;
    padding-top: 26px;
    border-top: 1px solid var(--line);
  }
  section:first-of-type {
    border-top: 0;
    padding-top: 0;
    margin-top: 28px;
  }
  .title-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 24px;
  }
  .desc {
    margin: 6px 0 0;
    color: var(--text-2);
    max-width: 64ch;
  }
  .folders {
    list-style: none;
    margin: 18px 0 0;
    padding: 0;
  }
  .folders li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 6px 8px 12px;
    border-radius: var(--r-sm);
    color: var(--text-3);
  }
  .folders li:hover {
    background: var(--hover);
  }
  .folders .none {
    color: var(--text-2);
  }
  .path {
    flex: 1;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 16px;
    margin-top: 10px;
    padding-left: 12px;
  }
  .small {
    font-size: 13px;
  }
  .link:disabled {
    color: var(--text-3);
    text-decoration: none;
  }
  .facts,
  .keys {
    display: grid;
    grid-template-columns: 140px 1fr;
    gap: 10px 20px;
    margin: 18px 0 0;
  }
  dt {
    color: var(--text-2);
  }
  dd {
    margin: 0;
  }
  .keys {
    grid-template-columns: 140px 1fr;
    align-items: center;
  }
  .keys dt {
    display: flex;
    gap: 5px;
  }
  kbd {
    display: inline-block;
    min-width: 26px;
    text-align: center;
    padding: 2px 7px;
    border-radius: 5px;
    background: var(--hover);
    box-shadow:
      inset 0 0 0 1px var(--line-strong),
      0 1px 0 var(--line-strong);
    color: var(--text);
    font: inherit;
    font-size: 12px;
  }
  .connected {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 16px;
  }
  .connected .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--good);
    box-shadow: 0 0 8px var(--good);
  }
  .connected .pill {
    margin-left: auto;
  }
  .connect {
    display: flex;
    gap: 10px;
    margin-top: 16px;
    max-width: 560px;
  }
  .connect .field {
    flex: 1;
    font-size: 14px;
    padding: 8px 12px;
  }
  .meter {
    height: 6px;
    max-width: 420px;
    margin-top: 16px;
    border-radius: 99px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .meter span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width 0.3s;
  }
  .hint {
    margin: 8px 0 0;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .error {
    margin: 8px 0 0;
    font-size: 13px;
    color: var(--warn);
  }
  .tip {
    display: flex;
    gap: 12px;
    margin-top: 18px;
    padding: 14px 16px;
    border-radius: var(--r-md);
    background: color-mix(in srgb, var(--accent) 9%, transparent);
    color: var(--accent);
  }
  .tip p {
    margin: 0;
    color: var(--text);
    font-size: 13px;
    line-height: 1.55;
  }
</style>

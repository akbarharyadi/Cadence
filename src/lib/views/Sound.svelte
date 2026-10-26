<script lang="ts">
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, db, EQ_BANDS, type Filter, type FilterKind, type LevelingMode, type Preset } from "$lib/api";
  import { autoHeadroom, formatHz } from "$lib/dsp";
  import { app } from "$lib/store.svelte";
  import EqGraph from "$lib/components/EqGraph.svelte";
  import Icon from "$lib/components/Icon.svelte";
  import Segmented from "$lib/components/Segmented.svelte";
  import Select from "$lib/components/Select.svelte";
  import SignalFlow from "$lib/components/SignalFlow.svelte";
  import Toggle from "$lib/components/Toggle.svelte";

  let presets = $state<Preset[]>([]);
  let levelingOptions = $state(false);
  let pasteOpen = $state(false);
  let pasteText = $state("");
  let importError = $state("");
  let selected = $state(-1);

  onMount(async () => {
    presets = await api.eqPresets();
  });

  const dsp = $derived(app.dsp);
  const eq = $derived(dsp?.eq);
  const headroom = $derived(eq?.enabled ? autoHeadroom(eq) : 0);
  const measured = $derived(app.tracks.filter((t) => t.gain.track).length);
  const sel = $derived(eq && eq.mode === "parametric" ? eq.filters[selected] : undefined);

  const levelingHelp: Record<Exclude<LevelingMode, "off">, string> = {
    auto: "Albums keep the loudness differences between their songs; shuffles and mixes are matched song by song.",
    album: "Every album plays at the same loudness, keeping the differences between its songs.",
    track: "Every song plays at the same loudness.",
  };
  const kinds: { value: FilterKind; label: string }[] = [
    { value: "peak", label: "Bell" },
    { value: "lowshelf", label: "Low shelf" },
    { value: "highshelf", label: "High shelf" },
    { value: "lowpass", label: "Low cut" },
    { value: "highpass", label: "High cut" },
  ];

  /** Direct output: WASAPI exclusive on Windows, device rate switching on macOS, ALSA hw on Linux, bit-perfect USB on Android. */
  const ua = typeof navigator !== "undefined" ? navigator.userAgent : "";
  const platform = ua.includes("Windows") ? "windows" : ua.includes("Android") ? "android" : ua.includes("Mac OS") ? "mac" : ua.includes("Linux") ? "linux" : "other";

  const tone = $derived(app.status?.output.tone ?? null);

  function scrollTo(id: string) {
    document.getElementById(`sound-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function setLeveling(on: boolean) {
    app.updateDsp((d) => (d.leveling.mode = on ? (d.leveling.mode === "off" ? "auto" : d.leveling.mode) : "off"));
  }

  function applyPreset(p: Preset) {
    app.updateDsp((d) => {
      d.eq.gains = [...p.gains];
      d.eq.preset = p.name;
      d.eq.enabled = true;
    });
  }

  function setBand(i: number, gain: number) {
    app.updateDsp((d) => {
      d.eq.gains[i] = gain;
      d.eq.preset = "Custom";
      d.eq.enabled = true;
    });
  }

  function editFilter(i: number, change: Partial<Filter>) {
    app.updateDsp((d) => {
      Object.assign(d.eq.filters[i], change);
      d.eq.enabled = true;
    });
  }

  function addFilter() {
    app.updateDsp((d) => {
      d.eq.filters.push({ kind: "peak", freq: 1000, gain: 3, q: 1, enabled: true });
      d.eq.enabled = true;
    });
    selected = (eq?.filters.length ?? 1) - 1;
  }

  function removeFilter(i: number) {
    app.updateDsp((d) => d.eq.filters.splice(i, 1));
    selected = -1;
  }

  async function importFile() {
    importError = "";
    const path = await open({ title: "Choose a ParametricEQ.txt", filters: [{ name: "AutoEq / Equalizer APO", extensions: ["txt"] }] });
    if (!path) return;
    const name = (path as string).split(/[\\/]/).pop()!.replace(/\s*ParametricEQ\.txt$/i, "").replace(/\.txt$/i, "");
    await doImport({ path: path as string }, name);
  }

  async function importPasted() {
    await doImport({ text: pasteText }, "Pasted profile");
    if (!importError) {
      pasteOpen = false;
      pasteText = "";
    }
  }

  async function doImport(args: { text?: string; path?: string }, profile: string) {
    try {
      const r = await api.parseEqFile(args);
      app.updateDsp((d) => {
        d.eq.mode = "parametric";
        d.eq.filters = r.filters;
        // AutoEq's preamp only provides headroom, which Cadence calculates itself.
        d.eq.preamp = 0;
        d.eq.profile = profile;
        d.eq.enabled = true;
      });
      selected = -1;
      app.showToast(`Loaded “${profile}” (${r.filters.length} filters)`);
    } catch (e) {
      importError = String(e);
    }
  }
</script>

<div class="page">
  <h1 class="page-title">Sound</h1>
  <p class="lead">How your music is shaped on its way to the speakers. Everything here is optional; with it all off, Cadence plays your files exactly as they are.</p>

  <SignalFlow onstage={scrollTo} />

  {#if dsp && eq}
    <!-- Direct output -->
    {#if platform !== "other"}
    <section id="sound-output">
      <div class="title-row">
        <div>
          <h2 class="section-title">Direct output</h2>
          {#if platform === "windows"}
            <p class="desc">
              Send music straight to your device at each song’s own sample rate, skipping the Windows mixer (WASAPI exclusive mode).
              With Sound Check, the equalizer and crossfeed off and volume at 100%, every sample arrives exactly as it is in the file.
            </p>
            <p class="hint">
              While Cadence plays, other apps can’t make sound on this device. Cadence lets go of it 20 seconds after you pause.
            </p>
          {:else if platform === "mac"}
            <p class="desc">
              Switch your output device to each song’s own sample rate, so macOS never resamples it (like setting the rate in Audio MIDI Setup, done for you).
              With Sound Check, the equalizer and crossfeed off and volume at 100%, every sample arrives exactly as it is in the file.
            </p>
            <p class="hint">Other apps keep playing, at the rate of the current song.</p>
          {:else if platform === "android"}
            <p class="desc">
              Send music straight to a USB DAC at each song’s own sample rate, without Android mixing, resampling or changing its volume.
              Phones with bit-perfect USB (Android 14 and later) do it themselves; on others Cadence drives the DAC with its own USB driver, after you allow it to use the DAC.
              With Sound Check, the equalizer and crossfeed off and volume at 100%, every sample arrives exactly as it is in the file.
            </p>
            <p class="hint">
              While Cadence drives the DAC, other apps’ sound comes from the phone’s speaker and the volume keys change Cadence’s volume. Cadence lets go of the DAC 20 seconds after you pause.
              Without a DAC it can use, Cadence plays through Android’s mixer, still at each song’s own rate.
            </p>
          {:else}
            <p class="desc">
              Play straight to your sound card (its ALSA hw device) at each song’s own sample rate, around PulseAudio or PipeWire.
              A USB DAC is used first. With Sound Check, the equalizer and crossfeed off and volume at 100%, every sample arrives exactly as it is in the file.
            </p>
            <p class="hint">
              The card has to be free: while another app holds it, Cadence plays through the system mixer instead. Cadence lets go of it 20 seconds after you pause.
            </p>
          {/if}
        </div>
        <Toggle checked={dsp.exclusive} label="Direct output" onchange={(v) => app.updateDsp((d) => (d.exclusive = v))} />
      </div>
    </section>
    {/if}

    <!-- Sound Check -->
    <section id="sound-leveling">
      <div class="title-row">
        <div>
          <h2 class="section-title">Sound Check</h2>
          <p class="desc">Plays every song at the same loudness, and gives loud masters the headroom they need to stay clean.</p>
        </div>
        <Toggle checked={dsp.leveling.mode !== "off"} label="Sound Check" onchange={setLeveling} />
      </div>
      {#if dsp.leveling.mode !== "off"}
        <button class="disclosure" onclick={() => (levelingOptions = !levelingOptions)} aria-expanded={levelingOptions}>
          <Icon name="chevron" size={14} />Options
        </button>
        {#if levelingOptions}
          <div class="options">
            <div class="opt">
              <span class="opt-label">Match</span>
              <Segmented
                label="Leveling mode"
                options={[{ value: "auto", label: "Automatic" }, { value: "album", label: "By album" }, { value: "track", label: "By song" }]}
                value={dsp.leveling.mode as "auto" | "album" | "track"}
                onchange={(v) => app.updateDsp((d) => (d.leveling.mode = v))}
              />
            </div>
            <p class="hint">{levelingHelp[dsp.leveling.mode as "auto" | "album" | "track"]}</p>
            <div class="opt">
              <span class="opt-label">Boost</span>
              <input type="range" class="slider" min="-6" max="6" step="0.5" value={dsp.leveling.preamp}
                style="--pct: {((dsp.leveling.preamp + 6) / 12) * 100}%; width: 260px"
                oninput={(e) => app.updateDsp((d) => (d.leveling.preamp = +e.currentTarget.value))} aria-label="Loudness boost" />
              <span class="num">{db(dsp.leveling.preamp)}</span>
            </div>
            <p class="hint">
              Reference level −18 LUFS (ReplayGain 2.0). Boosts never push a song past full scale.
              {#if app.analysis}Measuring {app.analysis.done} of {app.analysis.total} songs…{:else}{measured} of {app.tracks.length} songs measured.
                <button class="link" onclick={() => api.reanalyze()}>Measure again</button>{/if}
            </p>
          </div>
        {/if}
      {/if}
    </section>

    <!-- Tone per song -->
    <section id="sound-tone">
      <h2 class="section-title">Tone per song</h2>
      <p class="desc">Automatic adjustments made for each song, before your own equalizer.</p>
      <div class="tone-rows">
        <div class="tone-row">
          <div>
            <h3>Keep it full when quieter</h3>
            <p class="hint">
              When Sound Check turns a song down, adds back a little deep bass and top air, which our ears lose at lower volume
              (loudness compensation). The voice range is left untouched, so vocals stay forward.
            </p>
          </div>
          <Toggle checked={dsp.tone.loudness} label="Keep it full when quieter" onchange={(v) => app.updateDsp((d) => (d.tone.loudness = v))} />
        </div>
        <div class="tone-row">
          <div>
            <h3>Smart EQ</h3>
            <p class="hint">
              Cadence measures each song’s tonal balance and gently corrects it toward well-balanced music: a little air for dull songs,
              a little less boom for boomy ones. At most ±4 dB; songs of an album share one curve.
            </p>
          </div>
          <Toggle checked={dsp.tone.smart} label="Smart EQ" onchange={(v) => app.updateDsp((d) => (d.tone.smart = v))} />
        </div>
      </div>
      {#if tone}
        <div class="tone-now">
          <span class="tone-label">This song</span>
          {#if tone.bass > 0.05}<span class="tone-chip">Bass {db(tone.bass)}, treble {db(tone.treble)}</span>{/if}
          {#if tone.smart}
            <span class="tone-bars" aria-label="Smart EQ curve">
              {#each tone.smart as g, i}
                <span class="bar" title="{EQ_BANDS[i] >= 1000 ? `${EQ_BANDS[i] / 1000}k` : EQ_BANDS[i]} Hz: {db(g)}">
                  <span class="fill" class:neg={g < 0} style="height: {Math.min(Math.abs(g) / 2.5, 1) * 50}%"></span>
                </span>
              {/each}
            </span>
          {/if}
        </div>
      {/if}
    </section>

    <!-- Equalizer -->
    <section id="sound-eq">
      <div class="title-row">
        <div>
          <h2 class="section-title">Equalizer</h2>
          <p class="desc">Drag the points to shape the sound. The level is lowered automatically so boosts can’t distort.</p>
        </div>
        <Toggle checked={eq.enabled} label="Equalizer" onchange={(v) => app.updateDsp((d) => (d.eq.enabled = v))} />
      </div>

      <div class="eq-bar">
        <Segmented
          label="Equalizer type"
          options={[{ value: "graphic", label: "Presets" }, { value: "parametric", label: "Headphone correction" }]}
          value={eq.mode}
          onchange={(v) => {
            selected = -1;
            app.updateDsp((d) => (d.eq.mode = v));
          }}
        />
        <span class="headroom" title="Automatic level reduction that keeps boosts from clipping">
          {#if eq.enabled && headroom < 0}Level lowered by {db(-headroom).replace("+", "")}{:else}No level change{/if}
        </span>
      </div>

      <EqGraph {eq} bind:selected onband={setBand} onfilter={editFilter} />

      {#if eq.mode === "graphic"}
        <div class="chips" role="radiogroup" aria-label="Presets">
          {#each presets as p (p.name)}
            <button role="radio" aria-checked={eq.preset === p.name} class="chip" class:on={eq.preset === p.name} onclick={() => applyPreset(p)}>{p.name}</button>
          {/each}
          {#if eq.preset === "Custom"}<span class="chip on static">Custom</span>{/if}
        </div>
      {:else}
        <div class="correction">
          <div class="profile">
            <Icon name="headphones" size={20} />
            <div>
              <div class="profile-name">{eq.profile || "No headphone profile yet"}</div>
              <div class="hint">
                Get one for your headphones from
                <button class="link inline" onclick={() => openUrl("https://github.com/jaakkopasanen/AutoEq/tree/master/results")}>AutoEq</button>
                (find your model, then its ParametricEQ.txt). Equalizer APO files work too.
              </div>
            </div>
          </div>
          <div class="buttons">
            <button class="pill" onclick={importFile}><Icon name="folder" size={15} />Load file</button>
            <button class="pill ghost" onclick={() => { pasteOpen = !pasteOpen; importError = ""; }}>Paste text</button>
            <button class="pill ghost" onclick={addFilter}><Icon name="plus" size={15} />Add point</button>
          </div>
        </div>
        {#if pasteOpen}
          <textarea class="field paste" bind:value={pasteText} rows="5"
            placeholder={"Preamp: -6.1 dB\nFilter 1: ON LSC Fc 105 Hz Gain 6.4 dB Q 0.70\nFilter 2: ON PK Fc 205 Hz Gain -3.6 dB Q 0.43"}></textarea>
          <div class="buttons"><button class="pill" onclick={importPasted} disabled={!pasteText.trim()}>Use this profile</button></div>
        {/if}
        {#if importError}<p class="error">{importError}</p>{/if}

        {#if sel}
          <div class="editor">
            <span class="editor-title">Point {selected + 1}</span>
            <Select label="Filter shape" options={kinds} value={sel.kind} onchange={(v) => editFilter(selected, { kind: v })} width="140px" />
            <label class="numfield"><span>Frequency</span><input class="field" type="number" min="20" max="20000" value={sel.freq} onchange={(e) => editFilter(selected, { freq: +e.currentTarget.value })} /><em>Hz</em></label>
            <label class="numfield"><span>Gain</span><input class="field" type="number" min="-24" max="24" step="0.1" value={sel.gain} disabled={sel.kind === "lowpass" || sel.kind === "highpass"} onchange={(e) => editFilter(selected, { gain: +e.currentTarget.value })} /><em>dB</em></label>
            <label class="numfield"><span>Width (Q)</span><input class="field" type="number" min="0.1" max="20" step="0.05" value={sel.q} onchange={(e) => editFilter(selected, { q: +e.currentTarget.value })} /></label>
            <label class="check"><input type="checkbox" checked={sel.enabled} onchange={(e) => editFilter(selected, { enabled: e.currentTarget.checked })} />On</label>
            <button class="icon-btn" aria-label="Delete point {selected + 1}" title="Delete point" onclick={() => removeFilter(selected)}><Icon name="trash" size={16} /></button>
          </div>
        {:else if eq.filters.length}
          <p class="hint">Select a point to fine-tune it. Scroll over a point to make it wider or narrower; use the arrow keys for small steps.</p>
        {/if}
        {#if eq.filters.length}
          <div class="filter-list">
            {#each eq.filters as f, i (i)}
              <button class="fchip" class:on={selected === i} class:muted={!f.enabled} onclick={() => (selected = i)}>
                {kinds.find((k) => k.value === f.kind)?.label} {formatHz(f.freq)}{f.kind === "lowpass" || f.kind === "highpass" ? "" : `, ${db(f.gain)}`}
              </button>
            {/each}
          </div>
        {/if}
      {/if}
    </section>

    <!-- Crossfade -->
    <section id="sound-crossfade">
      <div class="title-row">
        <div>
          <h2 class="section-title">Crossfade</h2>
          <p class="desc">Blend the end of one song into the start of the next. Songs from the same album always play back to back without a gap, the way they were made.</p>
        </div>
        <Toggle checked={dsp.crossfade.enabled} label="Crossfade" onchange={(v) => app.updateDsp((d) => (d.crossfade.enabled = v))} />
      </div>
      {#if dsp.crossfade.enabled}
        <div class="opt">
          <span class="opt-label">Length</span>
          <input type="range" class="slider" min="1" max="12" step="1" value={dsp.crossfade.seconds}
            style="--pct: {((dsp.crossfade.seconds - 1) / 11) * 100}%; width: 260px"
            oninput={(e) => app.updateDsp((d) => (d.crossfade.seconds = +e.currentTarget.value))} aria-label="Crossfade length" />
          <span class="num">{dsp.crossfade.seconds} s</span>
        </div>
      {/if}
    </section>

    <!-- Crossfeed -->
    <section id="sound-crossfeed">
      <div class="title-row">
        <div>
          <h2 class="section-title">Headphone crossfeed</h2>
          <p class="desc">Lets each ear hear a little of the other channel, like speakers in a room. Makes hard-panned recordings easier to listen to on headphones.</p>
        </div>
        <Toggle checked={dsp.crossfeed.enabled} label="Crossfeed" onchange={(v) => app.updateDsp((d) => (d.crossfeed.enabled = v))} />
      </div>
      {#if dsp.crossfeed.enabled}
        <div class="opt">
          <span class="opt-label">Strength</span>
          <Segmented
            label="Crossfeed strength"
            options={[{ value: "default", label: "Subtle" }, { value: "cmoy", label: "Medium" }, { value: "meier", label: "Strong" }]}
            value={dsp.crossfeed.level}
            onchange={(v) => app.updateDsp((d) => (d.crossfeed.level = v))}
          />
        </div>
      {/if}
    </section>
  {/if}
</div>

<style>
  .page {
    max-width: 1040px;
  }
  .lead {
    margin: 10px 0 26px;
    color: var(--text-2);
    max-width: 64ch;
  }
  section {
    margin-top: 34px;
    padding-top: 28px;
    border-top: 1px solid var(--line);
    scroll-margin-top: 60px;
  }
  .title-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 28px;
  }
  .desc {
    margin: 6px 0 0;
    color: var(--text-2);
    max-width: 66ch;
  }
  .disclosure {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-top: 14px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-2);
  }
  .disclosure:hover {
    color: var(--text);
  }
  .disclosure :global(svg) {
    transition: transform 0.15s;
  }
  .disclosure[aria-expanded="true"] :global(svg) {
    transform: rotate(90deg);
  }
  .options {
    margin-top: 14px;
    padding-left: 20px;
    border-left: 2px solid var(--line-strong);
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-top: 14px;
  }
  .opt-label {
    width: 70px;
    font-size: 13px;
    color: var(--text-2);
  }
  .num {
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    width: 64px;
  }
  .hint {
    font-size: 12.5px;
    color: var(--text-3);
    margin: 8px 0 0;
    line-height: 1.5;
  }
  .eq-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 18px 0 14px;
  }
  .headroom {
    font-size: 12.5px;
    color: var(--text-3);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 16px;
  }
  .chip,
  .fchip {
    height: 30px;
    padding: 0 14px;
    border-radius: 999px;
    font-size: 13px;
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--line);
    color: var(--text-2);
    transition: background 0.12s, color 0.12s;
  }
  .chip:hover,
  .fchip:hover {
    background: var(--active);
    color: var(--text);
  }
  .chip.on,
  .fchip.on {
    background: var(--accent);
    color: var(--ink);
    box-shadow: none;
    font-weight: 600;
  }
  .chip.static {
    display: inline-flex;
    align-items: center;
  }
  .fchip.muted {
    text-decoration: line-through;
    opacity: 0.6;
  }
  /* Profile text and buttons side by side; the buttons move below when there's no room. */
  .correction {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px 20px;
    margin-top: 16px;
  }
  .profile {
    flex: 1 1 300px;
    display: flex;
    gap: 12px;
    align-items: flex-start;
    color: var(--text-2);
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .link.inline {
    display: inline;
    padding: 0;
    font-size: inherit;
  }
  .profile-name {
    font-weight: 600;
    color: var(--text);
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 10px;
  }
  .paste {
    width: 100%;
    margin-top: 14px;
    font-family: Consolas, monospace;
    resize: vertical;
  }
  .error {
    color: var(--warn);
    font-size: 13px;
  }
  .editor {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 14px;
    margin-top: 16px;
    padding: 12px 14px;
    border-radius: var(--r-md);
    background: var(--hover);
  }
  .editor-title {
    font-weight: 600;
    color: var(--accent);
  }
  .numfield {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .numfield input {
    width: 84px;
    font-variant-numeric: tabular-nums;
  }
  .numfield em {
    font-style: normal;
    color: var(--text-3);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }
  .filter-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 12px;
  }
  .fchip {
    height: 26px;
    font-size: 12px;
  }
  .tone-rows {
    display: flex;
    flex-direction: column;
    gap: 16px;
    margin-top: 18px;
  }
  .tone-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 24px;
  }
  .tone-row h3 {
    margin: 0;
    font-size: 14.5px;
    font-weight: 600;
  }
  .tone-row .hint {
    margin-top: 4px;
    max-width: 64ch;
  }
  .tone-now {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 12px;
    margin-top: 18px;
    font-size: 13px;
  }
  .tone-label {
    color: var(--text-3);
  }
  .tone-chip {
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--hover);
    color: var(--text-2);
  }
  .tone-bars {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 32px;
  }
  .bar {
    position: relative;
    width: 7px;
    height: 100%;
    border-radius: 2px;
    background: var(--hover);
  }
  .fill {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 50%;
    border-radius: 2px;
    background: var(--accent);
  }
  .fill.neg {
    bottom: auto;
    top: 50%;
    opacity: 0.6;
  }
</style>

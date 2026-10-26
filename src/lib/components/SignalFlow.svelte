<script lang="ts">
  // What happens to the music on its way to the speakers, as a row of stages.
  import { db, khz } from "$lib/api";
  import { app, signalPath, systemMixer } from "$lib/store.svelte";
  import Icon from "./Icon.svelte";

  let { onstage }: { onstage?: (id: string) => void } = $props();

  const s = $derived(app.status);
  const o = $derived(s?.output);
  const src = $derived(s?.source);
  const path = $derived(signalPath(s));
  const dsp = $derived(app.dsp);

  interface Stage {
    id: string;
    icon: string;
    name: string;
    value: string;
    active: boolean;
    /** Changes the samples (vs. a pass-through). */
    alters: boolean;
  }

  const stages = $derived.by<Stage[]>(() => {
    if (!o || !src) return [];
    const leveling = o.leveling;
    return [
      { id: "file", icon: "file", name: "Your file", value: `${src.codec} ${src.bits ? `${src.bits}-bit/` : ""}${khz(src.sampleRate)}`, active: true, alters: false },
      {
        id: "leveling",
        icon: "gauge",
        name: "Sound Check",
        value: leveling == null ? "Off" : db(leveling),
        active: leveling != null && leveling !== 0,
        alters: leveling != null && leveling !== 0,
      },
      {
        id: "tone",
        icon: "sparkle",
        name: "Tone",
        value: !o.tone ? "Off" : o.tone.bass > 0.05 && o.tone.smart ? "Both on" : o.tone.bass > 0.05 ? "Fuller" : "Smart EQ",
        active: !!o.tone,
        alters: !!o.tone,
      },
      {
        id: "eq",
        icon: "eq",
        name: "Equalizer",
        value: o.eq ? (dsp?.eq.mode === "parametric" ? dsp.eq.profile || "Parametric" : dsp?.eq.preset || "On") : "Off",
        active: o.eq,
        alters: o.eq,
      },
      { id: "crossfeed", icon: "headphones", name: "Crossfeed", value: o.crossfeed ? "On" : "Off", active: o.crossfeed, alters: o.crossfeed },
      {
        id: "rate",
        icon: "wave",
        name: "Sample rate",
        value: o.resampling ? `${khz(src.sampleRate)} → ${khz(o.sampleRate)}` : "Native",
        active: o.resampling,
        alters: o.resampling,
      },
      {
        id: "output",
        icon: "speaker",
        name: o.mode === "exclusive" ? `Direct, ${o.bits ?? 24}-bit/${khz(o.sampleRate)}` : systemMixer[0].toUpperCase() + systemMixer.slice(1),
        value: o.device.replace(/\s*\(.*\)\s*$/, ""),
        active: true,
        alters: false,
      },
    ];
  });

  const headline = $derived.by(() => {
    if (!path) return "";
    if (path.tier === "bitperfect") return "Bit-perfect: every sample reaches your device untouched";
    if (path.tier === "lossless") return "Untouched by Cadence";
    return "Shaped by Cadence";
  });
</script>

{#if stages.length && path}
  <div class="flow" class:playing={s?.playing}>
    <div class="head">
      <span class="tier {path.tier}">{path.label}</span>
      <span class="headline">{headline}</span>
      {#if o?.clipping}<span class="clip">The output reached full scale</span>{/if}
    </div>
    {#if o?.note}<p class="note">{o.note}</p>{/if}
    <ol class="stages">
      {#each stages as st, i (st.id)}
        {#if i > 0}<li class="wire" class:lit={st.active || stages[i - 1].active} aria-hidden="true"></li>{/if}
        <li>
          <button class="stage" class:active={st.active} class:alters={st.alters} onclick={() => onstage?.(st.id)} disabled={!onstage || st.id === "file" || st.id === "output"}>
            <span class="ico"><Icon name={st.icon} size={18} /></span>
            <span class="name">{st.name}</span>
            <span class="value" title={st.value}>{st.value}</span>
          </button>
        </li>
      {/each}
    </ol>
    {#if o}
      <div class="meter" aria-label="Output level">
        <div class="fill" class:hot={o.peakDb > -1} style="width: {Math.max(0, Math.min(100, ((o.peakDb + 48) / 48) * 100))}%"></div>
        <span>{o.peakDb > -100 ? `Peak ${db(o.peakDb)}FS` : "Silent"}</span>
      </div>
    {/if}
  </div>
{:else}
  <div class="flow empty">
    <Icon name="wave" size={20} />
    <span>Play a song to see what happens to it on the way to your speakers.</span>
  </div>
{/if}

<style>
  .flow {
    container-type: inline-size;
    padding: 20px 22px 18px;
    border-radius: var(--r-lg);
    background: color-mix(in srgb, var(--surface) 70%, transparent);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .flow.empty {
    display: flex;
    align-items: center;
    gap: 12px;
    color: var(--text-2);
  }
  .head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 12px;
    margin-bottom: 18px;
  }
  .tier {
    font-size: 12px;
    font-weight: 700;
    padding: 4px 10px;
    border-radius: 999px;
  }
  .tier.bitperfect,
  .tier.lossless {
    color: #1a1406;
    background: var(--gold);
  }
  .tier.enhanced {
    color: var(--ink);
    background: var(--accent);
  }
  .headline {
    font-family: var(--display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .clip {
    margin-left: auto;
    color: var(--bad);
    font-size: 13px;
    font-weight: 600;
  }
  .note {
    margin: -6px 0 14px;
    font-size: 13px;
    color: var(--warn);
  }
  .stages {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    align-items: stretch;
  }
  .stages li:not(.wire) {
    flex: 1 1 0;
    min-width: 0;
  }
  .wire {
    flex: 0 0 18px;
    align-self: center;
    height: 2px;
    background: var(--line-strong);
  }
  .wire.lit {
    background: linear-gradient(90deg, transparent, var(--accent), transparent);
    background-size: 200% 100%;
  }
  .playing .wire.lit {
    animation: flow 1.6s linear infinite;
  }
  /* Narrow windows: stages wrap into a grid instead of squeezing their text. */
  @container (max-width: 860px) {
    .stages {
      display: grid;
      grid-template-columns: repeat(3, 1fr);
      gap: 10px;
    }
    .wire {
      display: none;
    }
  }
  @keyframes flow {
    from {
      background-position: 100% 0;
    }
    to {
      background-position: -100% 0;
    }
  }
  .stage {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 12px 12px 11px;
    border-radius: var(--r-md);
    text-align: left;
    background: var(--hover);
    color: var(--text-3);
    transition: background 0.15s;
  }
  .stage:not(:disabled):hover {
    background: var(--active);
  }
  .stage.active {
    color: var(--text);
  }
  .stage.alters {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .ico {
    margin-bottom: 6px;
    color: var(--text-3);
  }
  .active .ico {
    color: var(--text-2);
  }
  .alters .ico {
    color: var(--accent);
  }
  .name {
    font-size: 12px;
    font-weight: 600;
  }
  .value {
    font-size: 12px;
    color: var(--text-2);
    max-width: 100%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .stage:not(.active) .value {
    color: var(--text-3);
  }
  .meter {
    position: relative;
    height: 6px;
    margin-top: 16px;
    border-radius: 3px;
    background: var(--line);
  }
  .meter .fill {
    height: 100%;
    border-radius: 3px;
    background: linear-gradient(90deg, var(--good), var(--good) 75%, var(--warn));
    transition: width 0.25s;
  }
  .meter .fill.hot {
    background: linear-gradient(90deg, var(--good), var(--warn) 70%, var(--bad));
  }
  .meter span {
    position: absolute;
    right: 0;
    top: 10px;
    font-size: 11px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
</style>

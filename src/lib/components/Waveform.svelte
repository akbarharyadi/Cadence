<script lang="ts">
  // The waveform behind a seek slider. Drawn once per song and size (an SVG mask, see waveform.ts);
  // playback only moves two transforms, exactly as the seek lines' fill does: the played copy sits
  // in a box slid left by the unplayed part (its overflow hidden), and is slid back by the same
  // amount inside it, so the picture stays put while the box uncovers it. No canvas redraw, no
  // animation frame, no relayout per update. The parent keeps its own slider (pointer and keys);
  // `shown` tells it when there's a waveform to show instead of the plain line.
  import { onDestroy } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { cachedWaveform, loadWaveform, waveMask, type Wave } from "#lib/waveform.js";

  let { trackId, fraction, shown = $bindable(false) }: { trackId: string; fraction: number; shown?: boolean } = $props();

  let wave = $state.raw<Wave | null>(null);
  let w = $state(0);
  let h = $state(0);
  // The size drawn at: follows the element's, a moment after it settles (a window being resized
  // draws once at the end, not at every step), and only when the whole-pixel size changed.
  let drawW = $state(0);
  let drawH = $state(0);
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const [nw, nh] = [Math.round(w), Math.round(h)];
    clearTimeout(timer);
    if (nw === drawW && nh === drawH) return;
    if (!drawW || !drawH) {
      drawW = nw;
      drawH = nh;
    } else {
      timer = setTimeout(() => {
        drawW = nw;
        drawH = nh;
      }, 200);
    }
  });

  // Asked for once per song (never per position update).
  let wanted = "";
  async function fetchWave(id: string) {
    wanted = id;
    const got = await loadWaveform(id);
    if (wanted === id) wave = got;
  }
  $effect(() => {
    const id = trackId;
    wanted = id;
    const known = cachedWaveform(id);
    wave = known;
    if (!known) void fetchWave(id);
  });

  // A song drawn in the background meanwhile: asked for once more.
  let unlisten: (() => void) | undefined;
  let gone = false;
  try {
    listen<{ trackId: string; drawn: boolean }>("waveform-ready", (e) => {
      if (e.payload?.drawn && e.payload.trackId === wanted && !wave) void fetchWave(wanted);
    })
      .then((u) => {
        if (!gone) unlisten = u;
        else void Promise.resolve(u()).catch(() => {});
      })
      .catch(() => {});
  } catch {
    // No backend to hear from: the plain line stays.
  }
  onDestroy(() => {
    gone = true;
    try {
      void Promise.resolve(unlisten?.()).catch(() => {});
    } catch {
      // Already gone with the window.
    }
    clearTimeout(timer);
  });

  const mask = $derived.by(() => {
    if (!wave || !drawW || !drawH) return null;
    try {
      return waveMask(wave, drawW, drawH);
    } catch {
      return null;
    }
  });
  $effect(() => {
    shown = !!mask;
  });
  onDestroy(() => (shown = false));

  const rest = $derived(Number.isFinite(fraction) ? (1 - Math.min(1, Math.max(0, fraction))) * 100 : 100);
</script>

<div class="wave" aria-hidden="true" bind:clientWidth={w} bind:clientHeight={h} style:--wave={mask} data-drawn={mask ? `${drawW}x${drawH}` : undefined}>
  {#if mask}
    <div class="layer rest"></div>
    <div class="played" style:transform="translateX(-{rest}%)">
      <div class="layer now" style:transform="translateX({rest}%)"></div>
    </div>
  {/if}
</div>

<style>
  .wave {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .layer {
    position: absolute;
    inset: 0;
    -webkit-mask: var(--wave) no-repeat 0 0 / 100% 100%;
    mask: var(--wave) no-repeat 0 0 / 100% 100%;
  }
  .rest {
    background: var(--wave-rest, rgba(255, 255, 255, 0.3));
  }
  .played {
    position: absolute;
    inset: 0;
    overflow: hidden;
    will-change: transform;
  }
  .now {
    background: var(--wave-played, var(--fill, white));
    will-change: transform;
  }
</style>

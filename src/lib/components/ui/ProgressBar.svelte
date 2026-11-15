<script lang="ts">
  // A job's progress: a fixed track with a bar scaled along it (`transform`, drawn by the
  // compositor), never resized (a width change lays the page out again), and no transition (a
  // job updates it every half second for hours; a transition restarted by each update never
  // ends and redraws at the display's rate). Size and colours come from the parent through
  // `--bar-height`, `--bar-radius`, `--bar-track` and `--bar-fill`.
  let { value, label, max, now }: { value: number; label?: string; max?: number; now?: number } = $props();
  const share = $derived(Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : 0);
</script>

<div class="track" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={max ?? 100} aria-valuenow={now ?? Math.round(share * 100)}>
  <div class="fill" data-progress-fill style="transform: scaleX({share})"></div>
</div>

<style>
  .track {
    height: var(--bar-height, 3px);
    border-radius: var(--bar-radius, 3px);
    background: var(--bar-track, var(--line-strong));
    overflow: hidden;
  }
  .fill {
    width: 100%;
    height: 100%;
    transform-origin: left;
    background: var(--bar-fill, var(--accent));
  }
  :global([dir="rtl"]) .fill {
    transform-origin: right;
  }
</style>

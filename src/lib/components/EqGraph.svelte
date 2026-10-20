<script lang="ts">
  // The equalizer as one picture you play with: drag the points on the curve.
  import { EQ_BANDS, db, type EqSettings, type Filter } from "$lib/api";
  import { formatFreq, formatHz, response } from "$lib/dsp";

  let {
    eq,
    selected = $bindable(-1),
    onband,
    onfilter,
  }: {
    eq: EqSettings;
    selected?: number;
    onband: (i: number, gain: number) => void;
    onfilter: (i: number, change: Partial<Filter>) => void;
  } = $props();

  const H = 260;
  const RANGE = 12;
  const PAD = 14;
  let W = $state(800);
  let svg: SVGSVGElement;
  let drag = $state<number | null>(null);
  let hover = $state<number | null>(null);

  const x = (f: number) => (Math.log10(f / 20) / 3) * W;
  const freqAt = (px: number) => 20 * Math.pow(1000, Math.min(1, Math.max(0, px / W)));
  const y = (g: number) => H / 2 - (Math.max(-RANGE - 3, Math.min(RANGE + 3, g)) / RANGE) * (H / 2 - PAD);
  const gainAt = (py: number) => Math.round(((H / 2 - py) / (H / 2 - PAD)) * RANGE * 2) / 2;

  const graphic = $derived(eq.mode === "graphic");
  /** The draggable points: graphic bands, or parametric filters that have a gain. */
  const points = $derived(
    graphic
      ? EQ_BANDS.map((f, i) => ({ i, freq: f, gain: eq.gains[i], y: eq.gains[i], label: formatHz(f), on: true }))
      : eq.filters.map((f, i) => {
          const gain = f.kind === "lowpass" || f.kind === "highpass" ? 0 : f.gain;
          // A shelf is at half its gain at its corner frequency, so its point sits on the curve there.
          const shelf = f.kind === "lowshelf" || f.kind === "highshelf";
          return { i, freq: f.freq, gain, y: shelf ? gain / 2 : gain, label: formatHz(f.freq), on: f.enabled };
        }),
  );

  const curve = $derived(response(eq, Math.max(120, Math.round(W / 4))).map((p) => `${x(p.freq).toFixed(1)},${y(p.db).toFixed(1)}`).join(" "));
  const area = $derived(`0,${H / 2} ${curve} ${W},${H / 2}`);
  const GRID = [50, 100, 200, 500, 1000, 2000, 5000, 10000];

  function local(e: PointerEvent) {
    const r = svg.getBoundingClientRect();
    return { px: ((e.clientX - r.left) / r.width) * W, py: ((e.clientY - r.top) / r.height) * H };
  }

  function down(e: PointerEvent, i: number) {
    e.preventDefault();
    drag = i;
    selected = i;
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  }

  function move(e: PointerEvent) {
    if (drag == null) return;
    const { px, py } = local(e);
    if (graphic) onband(drag, Math.max(-RANGE, Math.min(RANGE, gainAt(py))));
    else {
      const f = eq.filters[drag];
      const shelf = f.kind === "lowshelf" || f.kind === "highshelf";
      const gain = Math.max(-RANGE * 2, Math.min(RANGE * 2, gainAt(py) * (shelf ? 2 : 1)));
      const freq = Math.round(Math.max(20, Math.min(20000, freqAt(px))));
      onfilter(drag, f.kind === "lowpass" || f.kind === "highpass" ? { freq } : { freq, gain });
    }
  }

  function wheel(e: WheelEvent, i: number) {
    if (graphic) return;
    e.preventDefault();
    const q = eq.filters[i].q * (e.deltaY < 0 ? 1.1 : 1 / 1.1);
    onfilter(i, { q: Math.round(Math.max(0.1, Math.min(20, q)) * 100) / 100 });
  }

  function key(e: KeyboardEvent, i: number) {
    const step = e.shiftKey ? 2 : 0.5;
    const cur = points[i].gain;
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      e.preventDefault();
      const g = Math.max(-RANGE, Math.min(RANGE, cur + (e.key === "ArrowUp" ? step : -step)));
      if (graphic) onband(i, g);
      else onfilter(i, { gain: g });
    } else if (!graphic && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      e.preventDefault();
      const f = eq.filters[i].freq * (e.key === "ArrowRight" ? 1.06 : 1 / 1.06);
      onfilter(i, { freq: Math.round(Math.max(20, Math.min(20000, f))) });
    }
  }

  function reset(i: number) {
    if (graphic) onband(i, 0);
    else onfilter(i, { gain: 0 });
  }
</script>

<div class="wrap" class:off={!eq.enabled} bind:clientWidth={W}>
  <svg bind:this={svg} viewBox="0 0 {W} {H}" style="height: {H}px" onpointermove={move} onpointerup={() => (drag = null)} role="group" aria-label="Equalizer curve">
    <defs>
      <linearGradient id="eqfill" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="var(--accent)" stop-opacity="0.35" />
        <stop offset="0.5" stop-color="var(--accent)" stop-opacity="0.04" />
        <stop offset="1" stop-color="var(--accent)" stop-opacity="0.35" />
      </linearGradient>
    </defs>
    {#each GRID as f}
      <line x1={x(f)} x2={x(f)} y1="0" y2={H} class="grid" />
      {#if !graphic}<text x={x(f) + 5} y={H - 8} class="axis">{formatFreq(f)}</text>{/if}
    {/each}
    {#each [-12, -6, 6, 12] as g}
      <line x1="0" x2={W} y1={y(g)} y2={y(g)} class="grid" />
      <text x="8" y={y(g) - 4} class="axis">{g > 0 ? "+" : "−"}{Math.abs(g)}</text>
    {/each}
    <line x1="0" x2={W} y1={H / 2} y2={H / 2} class="zero" />
    <polygon points={area} fill="url(#eqfill)" />
    <polyline points={curve} class="curve" />

    {#each points as p (p.i)}
      {@const px = x(p.freq)}
      {@const py = y(p.y)}
      <g
        class="handle"
        class:dragging={drag === p.i}
        class:selected={selected === p.i && !graphic}
        class:disabled={!p.on}
        role="slider"
        tabindex="0"
        aria-label="{p.label} gain"
        aria-valuemin={-RANGE}
        aria-valuemax={RANGE}
        aria-valuenow={p.gain}
        aria-valuetext={db(p.gain)}
        onpointerdown={(e) => down(e, p.i)}
        onpointerenter={() => (hover = p.i)}
        onpointerleave={() => (hover = null)}
        ondblclick={() => reset(p.i)}
        onwheel={(e) => wheel(e, p.i)}
        onkeydown={(e) => key(e, p.i)}
        onfocus={() => (selected = p.i)}
      >
        {#if graphic}<line x1={px} x2={px} y1={H / 2} y2={py} class="stem" />{/if}
        <circle cx={px} cy={py} r="16" class="hit" />
        <circle cx={px} cy={py} r={drag === p.i ? 8 : 6.5} class="dot" />
        {#if hover === p.i || drag === p.i}
          {@const right = px < W - 120}
          <g class="tip" transform="translate({right ? px + 14 : px - 14}, {Math.max(18, py - 18)})">
            <text text-anchor={right ? "start" : "end"}>{p.label}  {db(p.gain)}{graphic ? "" : `  Q ${eq.filters[p.i].q}`}</text>
          </g>
        {/if}
      </g>
    {/each}
  </svg>
  {#if graphic}
    <div class="bands" aria-hidden="true">
      {#each EQ_BANDS as f}<span style="left: {x(f)}px">{formatFreq(f)}</span>{/each}
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    width: 100%;
    border-radius: var(--r-lg);
    background:
      radial-gradient(ellipse at 50% 50%, color-mix(in srgb, var(--accent) 7%, transparent), transparent 70%),
      rgba(8, 9, 16, 0.5);
    box-shadow: inset 0 0 0 1px var(--line);
    transition: opacity 0.2s;
  }
  .wrap.off {
    opacity: 0.55;
  }
  svg {
    display: block;
    width: 100%;
    touch-action: none;
    overflow: visible;
  }
  .grid {
    stroke: rgba(196, 200, 255, 0.05);
  }
  .zero {
    stroke: rgba(196, 200, 255, 0.2);
    stroke-dasharray: 3 5;
  }
  .axis {
    fill: var(--text-3);
    font-size: 10.5px;
    font-family: var(--font);
  }
  .curve {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2.5;
    stroke-linejoin: round;
    filter: drop-shadow(0 0 6px color-mix(in srgb, var(--accent) 60%, transparent));
  }
  .off .curve {
    stroke: var(--text-3);
    filter: none;
  }
  .handle {
    cursor: grab;
    outline: none;
  }
  .handle.dragging {
    cursor: grabbing;
  }
  .hit {
    fill: transparent;
  }
  .stem {
    stroke: color-mix(in srgb, var(--accent) 35%, transparent);
    stroke-width: 2;
  }
  .dot {
    fill: var(--ink);
    stroke: var(--accent);
    stroke-width: 2.5;
    transition: r 0.12s;
  }
  .handle:hover .dot,
  .handle:focus-visible .dot,
  .handle.selected .dot,
  .handle.dragging .dot {
    fill: var(--accent);
  }
  .handle:focus-visible .dot {
    stroke: white;
  }
  .handle.disabled .dot {
    stroke: var(--text-3);
    fill: var(--ink);
  }
  .tip text {
    fill: var(--text);
    font-size: 12px;
    font-weight: 600;
    font-family: var(--font);
    paint-order: stroke;
    stroke: var(--ink);
    stroke-width: 4px;
    stroke-linejoin: round;
    white-space: pre;
  }
  .bands {
    position: relative;
    height: 22px;
  }
  .bands span {
    position: absolute;
    transform: translateX(-50%);
    font-size: 11px;
    font-weight: 600;
    color: var(--text-2);
  }
</style>

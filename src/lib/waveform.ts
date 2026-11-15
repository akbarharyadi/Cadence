// The seek bar's waveform: the backend's stored envelope (src-tauri/src/waveform.rs), parsed, and
// drawn ONCE per song and size into an SVG used as a CSS mask. Playback progress never redraws it:
// the played copy is uncovered with transforms (see Waveform.svelte).
import { api } from "./api";

export interface Wave {
  /** Per bucket, 0..255 of full scale. */
  peak: Uint8Array;
  rms: Uint8Array | null;
}

const HEADER = 24;
/** Fewer buckets than this isn't worth drawing (a broken or tiny file). */
const MIN_BUCKETS = 16;

/** The backend's waveform file; null for anything that isn't a whole one of format 1. */
export function parseWaveform(data: ArrayBuffer | ArrayBufferView | number[] | null | undefined): Wave | null {
  if (!data) return null;
  let b: Uint8Array;
  if (data instanceof ArrayBuffer) b = new Uint8Array(data);
  else if (ArrayBuffer.isView(data)) b = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  else if (Array.isArray(data)) b = Uint8Array.from(data);
  else return null;
  if (b.length < HEADER || b[0] !== 0x43 || b[1] !== 0x44 || b[2] !== 0x57 || b[3] !== 0x46 || b[4] !== 1) return null;
  const n = b[6] | (b[7] << 8);
  const rows = b[5] & 1 ? 2 : 1;
  if (n < MIN_BUCKETS || b.length !== HEADER + n * rows) return null;
  return { peak: b.slice(HEADER, HEADER + n), rms: rows === 2 ? b.slice(HEADER + n, HEADER + 2 * n) : null };
}

/**
 * `count` bar heights (0..1). Normalised per song so a quiet master still reads and a loud one
 * isn't a solid block: the RMS (or the peak, without one) is scaled by the song's 95th
 * percentile, then a slight power curve brings out the difference between verse and chorus.
 */
export function barHeights(wave: Wave, count: number): number[] {
  const v = wave.rms ?? wave.peak;
  const n = v.length;
  if (count <= 0 || n === 0) return [];
  const sorted = Array.from(v).sort((a, b) => a - b);
  const p95 = sorted[Math.min(n - 1, Math.floor(n * 0.95))];
  const out: number[] = [];
  for (let j = 0; j < count; j++) {
    const a = Math.floor((j * n) / count);
    const z = Math.max(a + 1, Math.floor(((j + 1) * n) / count));
    let m = 0;
    for (let i = a; i < z && i < n; i++) m = Math.max(m, v[i]);
    out.push(p95 > 0 ? Math.pow(Math.min(1, m / p95), 1.3) : 0);
  }
  return out;
}

const BAR = 2;
const GAP = 1;

/**
 * The bars as a CSS `url(...)` of an SVG `width` × `height` CSS pixels: mirrored round-capped
 * 2 px bars with 1 px gaps. Null when there's no room to draw.
 */
export function waveMask(wave: Wave, width: number, height: number): string | null {
  const w = Math.floor(width);
  const h = Math.floor(height);
  if (w < 24 || h < 6) return null;
  const count = Math.floor((w + GAP) / (BAR + GAP));
  const heights = barHeights(wave, count);
  const left = (w - (count * (BAR + GAP) - GAP)) / 2;
  const mid = h / 2;
  let d = "";
  for (let j = 0; j < count; j++) {
    // At least a dot, so silence still shows where the song is.
    const len = Math.max(0, heights[j] * h - BAR);
    const x = left + j * (BAR + GAP) + BAR / 2;
    d += `M${+x.toFixed(1)} ${+(mid - len / 2).toFixed(1)}v${+len.toFixed(1)}`;
  }
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
    `<path d="${d}" stroke="#000" stroke-width="${BAR}" stroke-linecap="round" fill="none"/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}

// ---------- the last few songs' waveforms (current, previous, next at most) ----------

const KEEP = 3;
const cache = new Map<string, Wave>();

/** A song's waveform if it's in memory (no round trip, so the song change shows it at once). */
export function cachedWaveform(trackId: string): Wave | null {
  const known = cache.get(trackId);
  if (!known) return null;
  cache.delete(trackId);
  cache.set(trackId, known);
  return known;
}

/** A song's waveform: from memory, or asked for once. Null: not drawn yet (or can't be). */
export async function loadWaveform(trackId: string): Promise<Wave | null> {
  const known = cachedWaveform(trackId);
  if (known) return known;
  let wave: Wave | null = null;
  try {
    wave = parseWaveform(await api.waveform(trackId));
  } catch {
    wave = null;
  }
  if (wave) {
    cache.set(trackId, wave);
    while (cache.size > KEEP) cache.delete(cache.keys().next().value as string);
  }
  return wave;
}

/** Tests: start with nothing remembered. */
export function clearWaveforms() {
  cache.clear();
}

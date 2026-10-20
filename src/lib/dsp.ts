// Mirror of the engine's RBJ biquad design (src-tauri/src/audio/eq.rs), used to draw EQ curves.
import { EQ_BANDS, type EqSettings, type Filter } from "./api";

const GRAPHIC_Q = 1.41;

interface Coeffs {
  b0: number;
  b1: number;
  b2: number;
  a1: number;
  a2: number;
}

export function activeFilters(eq: EqSettings): Filter[] {
  if (eq.mode === "graphic") {
    return EQ_BANDS.map((freq, i) => ({ kind: "peak" as const, freq, gain: eq.gains[i] ?? 0, q: GRAPHIC_Q, enabled: true })).filter(
      (f) => f.gain !== 0,
    );
  }
  return eq.filters.filter((f) => f.enabled);
}

function design(f: Filter, rate: number): Coeffs {
  const A = Math.pow(10, f.gain / 40);
  const w0 = (2 * Math.PI * f.freq) / rate;
  const sin = Math.sin(w0);
  const cos = Math.cos(w0);
  const alpha = sin / (2 * Math.max(f.q, 0.01));
  let b0, b1, b2, a0, a1, a2;
  switch (f.kind) {
    case "peak":
      [b0, b1, b2, a0, a1, a2] = [1 + alpha * A, -2 * cos, 1 - alpha * A, 1 + alpha / A, -2 * cos, 1 - alpha / A];
      break;
    case "lowshelf": {
      const s = 2 * Math.sqrt(A) * alpha;
      [b0, b1, b2] = [A * (A + 1 - (A - 1) * cos + s), 2 * A * (A - 1 - (A + 1) * cos), A * (A + 1 - (A - 1) * cos - s)];
      [a0, a1, a2] = [A + 1 + (A - 1) * cos + s, -2 * (A - 1 + (A + 1) * cos), A + 1 + (A - 1) * cos - s];
      break;
    }
    case "highshelf": {
      const s = 2 * Math.sqrt(A) * alpha;
      [b0, b1, b2] = [A * (A + 1 + (A - 1) * cos + s), -2 * A * (A - 1 + (A + 1) * cos), A * (A + 1 + (A - 1) * cos - s)];
      [a0, a1, a2] = [A + 1 - (A - 1) * cos + s, 2 * (A - 1 - (A + 1) * cos), A + 1 - (A - 1) * cos - s];
      break;
    }
    case "lowpass":
      [b0, b1, b2, a0, a1, a2] = [(1 - cos) / 2, 1 - cos, (1 - cos) / 2, 1 + alpha, -2 * cos, 1 - alpha];
      break;
    case "highpass":
      [b0, b1, b2, a0, a1, a2] = [(1 + cos) / 2, -(1 + cos), (1 + cos) / 2, 1 + alpha, -2 * cos, 1 - alpha];
      break;
  }
  return { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 };
}

function magnitudeDb(c: Coeffs, freq: number, rate: number): number {
  const w = (2 * Math.PI * freq) / rate;
  const [c1, s1, c2, s2] = [Math.cos(w), -Math.sin(w), Math.cos(2 * w), -Math.sin(2 * w)];
  const nr = c.b0 + c.b1 * c1 + c.b2 * c2;
  const ni = c.b1 * s1 + c.b2 * s2;
  const dr = 1 + c.a1 * c1 + c.a2 * c2;
  const di = c.a1 * s1 + c.a2 * s2;
  return 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di));
}

/** Combined response (dB, including the manual preamp) at log-spaced points from 20 Hz to 20 kHz. */
export function response(eq: EqSettings, points = 240, rate = 48000): { freq: number; db: number }[] {
  const filters = activeFilters(eq)
    .filter((f) => f.freq > 0 && f.freq < rate * 0.45)
    .map((f) => design(f, rate));
  return Array.from({ length: points + 1 }, (_, i) => {
    const freq = 20 * Math.pow(1000, i / points);
    const db = filters.reduce((sum, c) => sum + magnitudeDb(c, freq, rate), eq.preamp);
    return { freq, db };
  });
}

/** The automatic level reduction the engine applies (dB, ≤ 0). */
export function autoHeadroom(eq: EqSettings): number {
  const top = Math.max(...response(eq, 480).map((p) => p.db));
  return top > 0 ? -top : 0;
}

/** "62 Hz", "1.3 kHz", "16 kHz". */
export const formatHz = (f: number) => (f >= 1000 ? `${+(f / 1000).toFixed(f % 1000 === 0 ? 0 : 1)} kHz` : `${Math.round(f)} Hz`);

export const formatFreq = (f: number) => (f >= 1000 ? `${+(f / 1000).toFixed(f % 1000 === 0 ? 0 : 1)}k` : `${Math.round(f)}`);

// How bright a cover is, so Now Playing darkens its blurred backdrop just enough for white text to
// stay readable on any cover: a white cover needs a lot of shade, a dark one almost none. Measured
// once per cover from its small artwork (24×24 pixels is plenty for an average).

const cache = new Map<string, number | null>();

/** sRGB channel (0–255) to linear light. */
const linear = (v: number) => {
  const c = v / 255;
  return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
};

/**
 * The cover's brightness as relative luminance (0 black – 1 white), from its brighter parts
 * (the 75th percentile: the blur spreads a bright area over the whole backdrop). Null when the
 * image can't be read.
 */
export async function coverLight(url: string): Promise<number | null> {
  if (!url) return null;
  if (cache.has(url)) return cache.get(url)!;
  let light: number | null = null;
  try {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.src = url;
    await img.decode();
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 24;
    const g = canvas.getContext("2d", { willReadFrequently: true })!;
    g.drawImage(img, 0, 0, 24, 24);
    const d = g.getImageData(0, 0, 24, 24).data;
    const values: number[] = [];
    for (let i = 0; i < d.length; i += 4) values.push(0.2126 * linear(d[i]) + 0.7152 * linear(d[i + 1]) + 0.0722 * linear(d[i + 2]));
    values.sort((a, b) => a - b);
    light = values[Math.floor(values.length * 0.75)];
  } catch {
    light = null;
  }
  cache.set(url, light);
  return light;
}

/** The backdrop's own filter keeps this share of the cover's brightness (brightness(0.7)). */
const FILTER = 0.7;
/** Behind the text the backdrop ends up no brighter than this (relative luminance, about #383838):
 *  white text then reads at 15:1, upcoming lyric lines at about 5:1. */
const TARGET = 0.04;

/** How much black goes over the backdrop (0–1) for a cover of this brightness. */
export function shadeFor(light: number | null): number {
  // Unknown (the image couldn't be read): enough for a light cover.
  if (light == null) return 0.62;
  const keep = (TARGET / Math.max(light, 1e-4)) ** (1 / 2.2);
  return Math.min(0.8, Math.max(0.12, 1 - keep / FILTER));
}

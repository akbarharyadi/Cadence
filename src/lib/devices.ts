// Output device names, as the Sound page shows them.

/** Windows names a device's output after what it is, the device in brackets: "Speakers (FiiO K7)". */
const GENERIC = new Set(["speakers", "speaker", "headphones", "headphone", "headset", "earphones", "digital audio", "digital output", "line out", "line", "spdif", "s/pdif", "output"]);

/** A device's name, fit to show: never empty, without control characters. */
export function deviceName(name: string | null | undefined): string {
  const clean = (name ?? "").replace(/[\u0000-\u001f\u007f]/g, "").trim();
  return clean || "Unnamed device";
}

/** The part that names the device: "Speakers (FiiO K7)" → "FiiO K7"; other names as they are. */
export function shortDeviceName(name: string | null | undefined): string {
  const full = deviceName(name);
  const m = /^(.*?)\s*\((.+)\)$/.exec(full);
  if (m && GENERIC.has(m[1].trim().toLowerCase()) && m[2].trim()) return m[2].trim();
  return full;
}

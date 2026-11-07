// Smart playlist rules: which songs match, in what order.
import type { HistoryStats, Mark, Track } from "./api";

export type Field =
  | "title"
  | "artist"
  | "albumArtist"
  | "album"
  | "genre"
  | "codec"
  | "year"
  | "rating"
  | "loved"
  | "plays"
  | "skips"
  | "lastPlayed"
  | "added"
  | "duration"
  | "sampleRate"
  | "bitDepth"
  | "bitrate"
  | "quality";
export type Op = "contains" | "notContains" | "is" | "isNot" | "gt" | "lt" | "inLast" | "notInLast";
export type Sort = "random" | "mostPlayed" | "topRated" | "recentlyPlayed" | "notPlayedLately" | "recentlyAdded" | "title" | "album";
/** The ratings and favourites by song id (app.marks). */
export type Marks = Record<string, Mark>;

export interface Rule {
  field: Field;
  op: Op;
  value: string;
}

export interface Smart {
  match: "all" | "any";
  rules: Rule[];
  /** Keep at most this many songs (0 = all). */
  limit: number;
  sort: Sort;
}

type Kind = "text" | "number" | "date" | "quality" | "flag";

export const FIELDS: { value: Field; label: string; kind: Kind }[] = [
  { value: "title", label: "Title", kind: "text" },
  { value: "artist", label: "Artist", kind: "text" },
  { value: "albumArtist", label: "Album artist", kind: "text" },
  { value: "album", label: "Album", kind: "text" },
  { value: "genre", label: "Genre", kind: "text" },
  { value: "codec", label: "Format (FLAC, MP3…)", kind: "text" },
  { value: "year", label: "Year", kind: "number" },
  { value: "rating", label: "Rating (stars, 0 = none)", kind: "number" },
  { value: "loved", label: "Favorite", kind: "flag" },
  { value: "plays", label: "Plays", kind: "number" },
  { value: "skips", label: "Skips", kind: "number" },
  { value: "duration", label: "Length (minutes)", kind: "number" },
  { value: "sampleRate", label: "Sample rate (kHz)", kind: "number" },
  { value: "bitDepth", label: "Bit depth", kind: "number" },
  { value: "bitrate", label: "Bitrate (kbps)", kind: "number" },
  { value: "lastPlayed", label: "Last played", kind: "date" },
  { value: "added", label: "Date added", kind: "date" },
  { value: "quality", label: "Quality", kind: "quality" },
];

export const YES_NO = [
  { value: "yes", label: "Yes" },
  { value: "no", label: "No" },
];

export const OPS: Record<Kind, { value: Op; label: string }[]> = {
  flag: [{ value: "is", label: "is" }],
  text: [
    { value: "contains", label: "contains" },
    { value: "notContains", label: "doesn’t contain" },
    { value: "is", label: "is" },
  ],
  number: [
    { value: "is", label: "is" },
    { value: "gt", label: "is more than" },
    { value: "lt", label: "is less than" },
  ],
  date: [
    { value: "inLast", label: "in the last (days)" },
    { value: "notInLast", label: "not in the last (days)" },
  ],
  quality: [
    { value: "is", label: "is" },
    { value: "isNot", label: "is not" },
  ],
};

export const QUALITIES = [
  { value: "hires", label: "Hi-Res Lossless" },
  { value: "lossless", label: "Lossless" },
  { value: "lossy", label: "Not lossless" },
];

export const SORTS: { value: Sort; label: string }[] = [
  { value: "random", label: "Random (changes daily)" },
  { value: "mostPlayed", label: "Most played" },
  { value: "topRated", label: "Top rated" },
  { value: "recentlyPlayed", label: "Recently played" },
  { value: "notPlayedLately", label: "Not played in the longest time" },
  { value: "recentlyAdded", label: "Recently added" },
  { value: "title", label: "Title" },
  { value: "album", label: "Album" },
];

export const kindOf = (f: Field) => FIELDS.find((x) => x.value === f)!.kind;

export function newSmart(): Smart {
  return { match: "all", rules: [{ field: "plays", op: "gt", value: "2" }], limit: 50, sort: "mostPlayed" };
}

function quality(t: Track): string {
  const lossless = ["FLAC", "ALAC", "WAV", "AIFF", "PCM"].includes(t.codec.toUpperCase());
  if (!lossless) return "lossy";
  return (t.sampleRate ?? 0) > 48000 ? "hires" : "lossless";
}

function test(t: Track, r: Rule, h: HistoryStats | null, marks: Marks, now: number): boolean {
  const kind = kindOf(r.field);
  if (kind === "flag") {
    const yes = !!marks[t.id]?.loved;
    return r.value === "no" ? !yes : yes;
  }
  if (kind === "text") {
    const v = (t[r.field as "title" | "artist" | "albumArtist" | "album" | "genre" | "codec"] ?? "").toLowerCase();
    const q = r.value.trim().toLowerCase();
    if (r.op === "contains") return v.includes(q);
    if (r.op === "notContains") return !v.includes(q);
    return v === q;
  }
  if (kind === "quality") {
    const q = quality(t);
    // "Lossless" includes hi-res, like a listener would expect.
    const match = r.value === "lossless" ? q !== "lossy" : q === r.value;
    return r.op === "isNot" ? !match : match;
  }
  if (kind === "date") {
    const ts = r.field === "added" ? t.added : (h?.lastPlayed[t.id] ?? 0);
    const days = Number(r.value) || 0;
    const within = ts > 0 && now - ts <= days * 86400;
    return r.op === "inLast" ? within : !within;
  }
  const n = numberOf(t, r.field, h, marks);
  const v = Number(r.value);
  if (!isFinite(v)) return true;
  if (r.op === "gt") return n > v;
  if (r.op === "lt") return n < v;
  return Math.round(n) === v;
}

function numberOf(t: Track, f: Field, h: HistoryStats | null, marks: Marks): number {
  switch (f) {
    case "year":
      return t.year ?? 0;
    case "plays":
      return h?.plays[t.id] ?? 0;
    case "skips":
      return h?.skips[t.id] ?? 0;
    case "rating":
      return marks[t.id]?.rating ?? 0;
    case "sampleRate":
      return (t.sampleRate ?? 0) / 1000;
    case "bitDepth":
      return t.bitDepth ?? 0;
    case "bitrate":
      return t.bitrate ?? 0;
    default:
      return t.duration / 60;
  }
}

/** Deterministic shuffle so a "random" playlist keeps its order for the day. */
function seededShuffle<T>(list: T[], seed: string): T[] {
  let h = 2166136261;
  for (const c of seed) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  const rand = () => {
    h = Math.imul(h ^ (h >>> 15), 2246822507);
    h = Math.imul(h ^ (h >>> 13), 3266489909);
    return ((h ^= h >>> 16) >>> 0) / 4294967296;
  };
  const a = [...list];
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}

export function evaluate(smart: Smart, tracks: Track[], history: HistoryStats | null, marks: Marks, seed = ""): Track[] {
  const now = Date.now() / 1000;
  const rules = smart.rules.filter((r) => r.value.trim() !== "");
  let out = rules.length
    ? tracks.filter((t) => (smart.match === "all" ? rules.every((r) => test(t, r, history, marks, now)) : rules.some((r) => test(t, r, history, marks, now))))
    : [...tracks];
  const plays = (t: Track) => history?.plays[t.id] ?? 0;
  const last = (t: Track) => history?.lastPlayed[t.id] ?? 0;
  const stars = (t: Track) => marks[t.id]?.rating ?? 0;
  switch (smart.sort) {
    case "random":
      out = seededShuffle(out, seed + new Date().toDateString());
      break;
    case "mostPlayed":
      out.sort((a, b) => plays(b) - plays(a) || last(b) - last(a));
      break;
    case "topRated":
      out.sort((a, b) => stars(b) - stars(a) || plays(b) - plays(a) || last(b) - last(a));
      break;
    case "recentlyPlayed":
      out.sort((a, b) => last(b) - last(a));
      break;
    case "notPlayedLately":
      // Never played first, then the longest ago.
      out.sort((a, b) => last(a) - last(b));
      break;
    case "recentlyAdded":
      out.sort((a, b) => b.added - a.added);
      break;
    case "title":
      out.sort((a, b) => a.title.localeCompare(b.title));
      break;
    case "album":
      out.sort((a, b) => a.album.localeCompare(b.album) || (a.discNo ?? 1) - (b.discNo ?? 1) || (a.trackNo ?? 0) - (b.trackNo ?? 0));
      break;
  }
  return smart.limit > 0 ? out.slice(0, smart.limit) : out;
}

/** A one-line description of the rules, for the playlist header. */
export function describe(smart: Smart): string {
  const parts = smart.rules
    .filter((r) => r.value.trim())
    .map((r) => {
      const f = FIELDS.find((x) => x.value === r.field)!;
      const op = OPS[f.kind].find((o) => o.value === r.op)?.label ?? r.op;
      const v = f.kind === "quality" ? QUALITIES.find((q) => q.value === r.value)?.label : f.kind === "flag" ? YES_NO.find((q) => q.value === r.value)?.label.toLowerCase() : r.value;
      return `${f.label.replace(/\s*\(.*\)/, "").toLowerCase()} ${op.replace(" (days)", "")} ${v}${f.kind === "date" ? " days" : ""}`;
    });
  if (!parts.length) return "All songs";
  return parts.join(smart.match === "all" ? ", and " : ", or ");
}

// The extra words that find songs, albums and artists by their other names (worked out by the app
// once per library: an artist's spellings and aliases, titles read in Latin letters). Search and
// the command palette match them after the names themselves, so they only ever add results.
import type { Album, SearchKeys, Track } from "./api";

interface Index {
  credits: [string, string][];
  tracks: Map<string, string>;
  albums: Map<string, string>;
}

// Converted once per set of keys (they're replaced whole when the library changes).
const indexes = new WeakMap<SearchKeys, Index>();
function indexOf(keys: SearchKeys): Index {
  let ix = indexes.get(keys);
  if (!ix) {
    ix = {
      credits: Object.entries(keys.credits ?? {}),
      tracks: new Map(Object.entries(keys.tracks ?? {})),
      albums: new Map(Object.entries(keys.albums ?? {})),
    };
    indexes.set(keys, ix);
  }
  return ix;
}

export interface ExtraMatch {
  track: (t: Track) => boolean;
  album: (a: Album) => boolean;
  /** An artist by any of its names (the page's and its other spellings). */
  artist: (a: { name: string; also?: string[] }) => boolean;
}

/** What the folded text `q` finds by other names; null when there are no keys (yet). One pass over
 *  the artist credits per query, then a lookup per item. */
export function extraMatch(keys: SearchKeys | null | undefined, q: string): ExtraMatch | null {
  if (!keys || !q) return null;
  const ix = indexOf(keys);
  const credits = new Set<string>();
  for (const [credit, words] of ix.credits) if (words.includes(q)) credits.add(credit);
  const albums = new Set<string>();
  for (const [id, words] of ix.albums) if (words.includes(q)) albums.add(id);
  return {
    track: (t) => credits.has(t.artist) || credits.has(t.albumArtist) || albums.has(t.albumId) || (ix.tracks.get(t.id)?.includes(q) ?? false),
    album: (a) => credits.has(a.artist) || albums.has(a.id),
    artist: (a) => credits.has(a.name) || (a.also ?? []).some((n) => credits.has(n)),
  };
}

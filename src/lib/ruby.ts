// Readings above the characters (furigana, pinyin) together with the karaoke sweep: the sung
// line is drawn piece by piece (a word, or a character in Japanese), and a reading that covers
// several pieces (今日 over 今 and 日) can't be split, so those pieces are swept as one.
import type { RubyPart } from "./api";

export interface Timed {
  text: string;
  start: number;
  end: number;
}

export interface RubyPiece extends Timed {
  /** The piece's text in parts, each with its reading or none. */
  parts: RubyPart[];
}

/** The pieces with their readings; each piece as plain text when the readings don't fit the line. */
export function rubyPieces(pieces: Timed[], parts: RubyPart[] | undefined): RubyPiece[] {
  const plain = () => pieces.map((p) => ({ ...p, parts: [{ text: p.text }] }));
  if (!parts?.length || !parts.some((p) => p.ruby)) return plain();
  const pieceText = pieces.map((p) => p.text).join("");
  const partText = parts.map((p) => p.text).join("");
  // Word timings can leave out what comes before the first word (a singer's name): line up from
  // where the pieces start in the line, or not at all.
  // (Spaces around the words don't count: the line itself is trimmed.)
  const core = pieceText.trim();
  const found = core ? partText.indexOf(core) : -1;
  if (found < 0) return plain();
  const lead = found - (pieceText.length - pieceText.trimStart().length);

  // Each part's place in the pieces' text.
  const spans: { from: number; to: number; part: RubyPart }[] = [];
  let at = -lead;
  for (const part of parts) {
    spans.push({ from: at, to: at + part.text.length, part });
    at += part.text.length;
  }
  // Pieces in groups that no reading crosses.
  const groups: { from: number; to: number; start: number; end: number; text: string }[] = [];
  let pos = 0;
  for (const p of pieces) {
    const from = pos;
    const to = pos + p.text.length;
    pos = to;
    const last = groups.at(-1);
    const crossed = !!last && spans.some((s) => s.part.ruby && s.from < last.to && s.to > last.to);
    if (last && crossed) {
      last.to = to;
      last.end = Math.max(last.end, p.end);
      last.text += p.text;
    } else groups.push({ from, to, start: p.start, end: p.end, text: p.text });
  }
  return groups.map((g) => {
    const out: RubyPart[] = [];
    for (const s of spans) {
      const from = Math.max(s.from, g.from);
      const to = Math.min(s.to, g.to);
      if (to <= from) continue;
      if (s.part.ruby && from === s.from && to === s.to) out.push({ text: s.part.text, ruby: s.part.ruby });
      else out.push({ text: s.part.text.slice(from - s.from, to - s.from) });
    }
    // What the readings don't cover (spaces around the words) as it is.
    const joined = out.map((p) => p.text).join("");
    if (joined !== g.text) {
      if (joined && g.text.startsWith(joined)) out.push({ text: g.text.slice(joined.length) });
      else if (joined && g.text.endsWith(joined)) out.unshift({ text: g.text.slice(0, g.text.length - joined.length) });
      else return { start: g.start, end: g.end, text: g.text, parts: [{ text: g.text }] };
    }
    return { start: g.start, end: g.end, text: g.text, parts: out };
  });
}

/** Readings for lines, checked: a line's parts must spell the line exactly, else it has none. */
export function checkedRuby(lines: string[], ruby: RubyPart[][] | null | undefined): (RubyPart[] | undefined)[] {
  return lines.map((line, i) => {
    const parts = Array.isArray(ruby?.[i]) ? ruby![i] : undefined;
    if (!parts?.length || !parts.every((p) => typeof p?.text === "string")) return undefined;
    return parts.map((p) => p.text).join("") === line ? parts : undefined;
  });
}

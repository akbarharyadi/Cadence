#!/usr/bin/env node
// Size check for the source tree: files and functions over the limits in AGENTS.md.
//
//   node scripts/check-size.mjs                   violations only (exit 1 if there are any)
//   node scripts/check-size.mjs --all             also list everything over the soft target
//   node scripts/check-size.mjs --update-allowlist  rewrite scripts/size-allowlist.json from the tree
//
// No dependencies. The scanner is an approximation (see "blind spots" at the end of this file).
// Existing code over a cap is listed in scripts/size-allowlist.json: it may not grow past its
// recorded size plus a small tolerance; everything else must stay under the cap.

import { readdirSync, readFileSync, writeFileSync, statSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const ALLOWLIST = join(ROOT, "scripts", "size-allowlist.json");

// Limits: `target` is where well-factored code sits, `cap` is the most a new file or function may
// have. Keep these numbers in step with the table in AGENTS.md.
export const LIMITS = {
  files: {
    rust: { target: 500, cap: 800 }, // lines of code, not counting #[cfg(test)] blocks
    svelte: { target: 350, cap: 700 }, // whole file: script + markup + style
    ts: { target: 250, cap: 500 }, // modules (src/lib, src/routes)
    css: { target: 600, cap: 800 },
    test: { target: 300, cap: 500 }, // src/tests/*.test.ts and helpers
  },
  functions: {
    rust: { target: 50, cap: 100 },
    ts: { target: 40, cap: 60 }, // functions, methods, and arrow functions given a name
    svelte: { target: 40, cap: 60 }, // the same, plus $effect / $derived.by / onMount blocks
  },
  svelteScript: { target: 150, cap: 300 }, // the <script> part of one component, in lines
  effects: { target: 3, cap: 6 }, // $effect blocks in one component
  tolerance: { percent: 5, minLines: 20 }, // a grandfathered entry may grow by this much
};

const SKIP_DIRS = new Set(["node_modules", "target", "build", ".svelte-kit", "gen", "dist", ".git"]);
const ROOTS = ["src", "src-tauri/src"];

// ---------------------------------------------------------------- masking

/** Blank out comments and string contents (keeping newlines and the quotes), so braces and `fn`
 *  inside them are not counted. Output has the same length and line breaks as the input. */
function maskRust(src) {
  const out = src.split("");
  const n = src.length;
  const blank = (a, b) => {
    for (let k = a; k < b; k++) if (out[k] !== "\n") out[k] = " ";
  };
  let i = 0;
  while (i < n) {
    const c = src[i];
    const d = src[i + 1];
    if (c === "/" && d === "/") {
      let j = i;
      while (j < n && src[j] !== "\n") j++;
      blank(i, j);
      i = j;
    } else if (c === "/" && d === "*") {
      let depth = 1;
      let j = i + 2;
      while (j < n && depth > 0) {
        if (src[j] === "/" && src[j + 1] === "*") (depth++, (j += 2));
        else if (src[j] === "*" && src[j + 1] === "/") (depth--, (j += 2));
        else j++;
      }
      blank(i, j);
      i = j;
    } else if ((c === "r" || (c === "b" && d === "r")) && /^(?:b?r)(#*)"/.test(src.slice(i, i + 12)) && !/\w/.test(src[i - 1] || "")) {
      const m = /^(?:b?r)(#*)"/.exec(src.slice(i, i + 12));
      const close = '"' + m[1];
      const start = i + m[0].length;
      const end = src.indexOf(close, start);
      const stop = end < 0 ? n : end + close.length;
      blank(start, end < 0 ? n : end);
      i = stop;
    } else if (c === '"') {
      let j = i + 1;
      while (j < n && src[j] !== '"') j += src[j] === "\\" ? 2 : 1;
      blank(i + 1, j);
      i = j + 1;
    } else if (c === "'") {
      // A char literal ('x', '\n', '\u{..}') or a lifetime ('a). A literal closes within a few chars.
      if (d === "\\") {
        const end = src.indexOf("'", i + 3);
        if (end > 0 && end - i < 12) {
          blank(i + 1, end);
          i = end + 1;
          continue;
        }
      } else if (src[i + 2] === "'" && d !== "\n") {
        blank(i + 1, i + 2);
        i += 3;
        continue;
      } else if (d && /[\uD800-\uDBFF]/.test(d) && src[i + 3] === "'") {
        blank(i + 1, i + 3);
        i += 4;
        continue;
      }
      i++;
    } else i++;
  }
  return out.join("");
}

/** The same for TypeScript / JavaScript (regex literals are guessed from the previous token). */
function maskTs(src) {
  const out = src.split("");
  const n = src.length;
  const blank = (a, b) => {
    for (let k = a; k < b; k++) if (out[k] !== "\n") out[k] = " ";
  };
  const stack = []; // brace depth markers for `${` inside template literals
  let depth = 0;
  let prev = ""; // the previous significant character, to tell a regex from a division
  let prevWord = "";
  let i = 0;
  const template = (from) => {
    let j = from;
    while (j < n) {
      if (src[j] === "\\") j += 2;
      else if (src[j] === "`") {
        blank(from, j);
        return j + 1;
      } else if (src[j] === "$" && src[j + 1] === "{") {
        blank(from, j);
        stack.push(depth);
        depth++;
        prev = "{";
        return j + 2;
      } else j++;
    }
    blank(from, n);
    return n;
  };
  while (i < n) {
    const c = src[i];
    const d = src[i + 1];
    if (c === "/" && d === "/") {
      let j = i;
      while (j < n && src[j] !== "\n") j++;
      blank(i, j);
      i = j;
    } else if (c === "/" && d === "*") {
      const e = src.indexOf("*/", i + 2);
      const j = e < 0 ? n : e + 2;
      blank(i, j);
      i = j;
    } else if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < n && src[j] !== c && src[j] !== "\n") j += src[j] === "\\" ? 2 : 1;
      blank(i + 1, j);
      i = j + 1;
      prev = c;
    } else if (c === "`") {
      i = template(i + 1);
      if (src[i - 1] === "`") prev = "`";
    } else if (c === "/" && (prev === "" || "(,=:[!&|?{};".includes(prev) || prevWord === "return")) {
      // A regex literal.
      let j = i + 1;
      let cls = false;
      while (j < n && src[j] !== "\n") {
        if (src[j] === "\\") j += 2;
        else if (src[j] === "[") (cls = true, j++);
        else if (src[j] === "]") (cls = false, j++);
        else if (src[j] === "/" && !cls) break;
        else j++;
      }
      blank(i + 1, j);
      i = j + 1;
      prev = "/";
    } else {
      if (c === "{") depth++;
      else if (c === "}") {
        depth--;
        if (stack.length && stack[stack.length - 1] === depth) {
          stack.pop();
          i = template(i + 1);
          continue;
        }
      }
      if (/\s/.test(c)) {
        i++;
        continue;
      }
      if (/\w/.test(c)) {
        let j = i;
        while (j < n && /[\w$]/.test(src[j])) j++;
        prevWord = src.slice(i, j);
        prev = "a";
        i = j;
        continue;
      }
      prev = c;
      prevWord = "";
      i++;
    }
  }
  return out.join("");
}

// ---------------------------------------------------------------- scanning helpers

function lineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i++) if (text[i] === "\n") starts.push(i + 1);
  return starts;
}
function lineOf(starts, pos) {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= pos) lo = mid;
    else hi = mid - 1;
  }
  return lo + 1;
}
/** Index of the `}` that closes the `{` at `open`, or -1. */
function matchBrace(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    if (c === "{") depth++;
    else if (c === "}" && --depth === 0) return i;
  }
  return -1;
}
/** Index just after the `)` that closes the `(` at `open`. */
function skipParens(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    if (text[i] === "(") depth++;
    else if (text[i] === ")" && --depth === 0) return i + 1;
  }
  return -1;
}

// ---------------------------------------------------------------- Rust

function scanRust(src) {
  const masked = maskRust(src);
  const starts = lineStarts(masked);
  const total = starts.length - (src.endsWith("\n") ? 1 : 0);
  // #[cfg(test)] items at the start of a line: their lines don't count as code.
  const testRanges = [];
  const re = /^#\[cfg\(test\)\]/gm;
  let m;
  while ((m = re.exec(masked))) {
    const open = masked.indexOf("{", m.index);
    const semi = masked.indexOf(";", m.index);
    if (open < 0 || (semi >= 0 && semi < open)) continue; // `#[cfg(test)] use ...;`
    const close = matchBrace(masked, open);
    if (close < 0) continue;
    testRanges.push([lineOf(starts, m.index), lineOf(starts, close)]);
    re.lastIndex = close;
  }
  const inTest = (line) => testRanges.some(([a, b]) => line >= a && line <= b);
  const testLines = testRanges.reduce((s, [a, b]) => s + (b - a + 1), 0);
  const fns = [];
  const fre = /\bfn\s+([A-Za-z_]\w*)/g;
  while ((m = fre.exec(masked))) {
    let i = m.index + m[0].length;
    // Skip generics and the parameter list, then look for the body `{` or a `;` (no body).
    let paramsStart = masked.indexOf("(", i);
    if (paramsStart < 0) break;
    const afterParams = skipParens(masked, paramsStart);
    if (afterParams < 0) continue;
    const params = masked.slice(paramsStart + 1, afterParams - 1);
    let j = afterParams;
    let angle = 0;
    while (j < masked.length) {
      const c = masked[j];
      if (c === "<") angle++;
      else if (c === ">" && masked[j - 1] !== "-") angle--;
      else if (c === "(") {
        j = skipParens(masked, j) - 1;
      } else if ((c === "{" && angle <= 0) || (c === ";" && angle <= 0)) break;
      j++;
    }
    if (masked[j] !== "{") continue;
    const close = matchBrace(masked, j);
    if (close < 0) continue;
    const a = lineOf(starts, m.index);
    const b = lineOf(starts, close);
    if (inTest(a)) continue;
    fns.push({ name: m[1], line: a, lines: b - a + 1, params: countParams(params) });
  }
  return { total, code: total - testLines, fns, balanced: true };
}

function countParams(list) {
  const t = list.trim();
  if (!t) return 0;
  let depth = 0;
  let n = 1;
  for (const c of t) {
    if ("<([{".includes(c)) depth++;
    else if (">)]}".includes(c)) depth--;
    else if (c === "," && depth === 0) n++;
  }
  if (t.endsWith(",")) n--;
  return n;
}

// ---------------------------------------------------------------- TS and the script part of Svelte

const NOT_METHODS = new Set(["if", "for", "while", "switch", "catch", "function", "return", "with", "else", "do", "await", "typeof", "new"]);
const CALLBACKS = ["\\$effect(?:\\.pre|\\.root)?", "\\$derived\\.by", "onMount", "onDestroy", "beforeNavigate", "afterNavigate"];

/** Finds functions in masked TS. `lineBase` shifts line numbers (Svelte script blocks). */
function scanTsFunctions(masked, lineBase = 0) {
  const starts = lineStarts(masked);
  const found = [];
  const seenBodies = new Set();
  const add = (name, at, bodyOpen) => {
    if (bodyOpen < 0 || masked[bodyOpen] !== "{" || seenBodies.has(bodyOpen)) return;
    const close = matchBrace(masked, bodyOpen);
    if (close < 0) return;
    seenBodies.add(bodyOpen);
    const a = lineOf(starts, at);
    const b = lineOf(starts, close);
    found.push({ name, line: a + lineBase, lines: b - a + 1 });
  };
  /** After `=>`: a block body, or an expression that runs to the end of the statement. */
  const arrowBody = (name, at, arrow) => {
    let j = arrow + 2;
    while (/\s/.test(masked[j] || "")) j++;
    if (masked[j] === "{") return add(name, at, j);
    let depth = 0;
    let k = j;
    for (; k < masked.length; k++) {
      const c = masked[k];
      if ("([{".includes(c)) depth++;
      else if (")]}".includes(c)) {
        if (--depth < 0) break;
      } else if ((c === ";" || c === ",") && depth === 0) break;
    }
    const a = lineOf(starts, at);
    const b = lineOf(starts, Math.max(at, k - 1));
    if (b - a + 1 >= 8) found.push({ name, line: a + lineBase, lines: b - a + 1 });
  };
  let m;
  // function name(...) { ... }
  const f1 = /\bfunction\s*\*?\s*([A-Za-z_$][\w$]*)\s*(?:<[^(){}]*>)?\s*\(/g;
  while ((m = f1.exec(masked))) {
    const close = skipParens(masked, m.index + m[0].length - 1);
    if (close < 0) continue;
    const open = masked.indexOf("{", close);
    add(m[1], m.index, open);
  }
  // const name = (async)? (...) => ... | function ... | x => ...
  const f2 = /\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=;{}()]+)?=\s*(async\s+)?/g;
  while ((m = f2.exec(masked))) {
    let j = m.index + m[0].length;
    if (/^function\b/.test(masked.slice(j, j + 9))) {
      const open0 = masked.indexOf("(", j);
      const close = open0 < 0 ? -1 : skipParens(masked, open0);
      if (close > 0) add(m[1], m.index, masked.indexOf("{", close));
    } else if (masked[j] === "(") {
      const close = skipParens(masked, j);
      if (close < 0) continue;
      const rest = masked.slice(close, close + 120);
      const arrow = /^\s*(?::[^=]*?)?=>/.exec(rest);
      if (arrow) arrowBody(m[1], m.index, close + arrow[0].length - 2);
    } else {
      const ident = /^[A-Za-z_$][\w$]*\s*=>/.exec(masked.slice(j, j + 60));
      if (ident) arrowBody(m[1], m.index, j + ident[0].length - 2);
    }
  }
  // class and object methods: `name(args) {` at the start of a line
  const f3 = /^[ \t]*(?:(?:public|private|protected|static|async|get|set|readonly)\s+)*([A-Za-z_$][\w$]*)\s*(?:<[^(){}]*>)?\s*\(/gm;
  while ((m = f3.exec(masked))) {
    if (NOT_METHODS.has(m[1])) continue;
    const close = skipParens(masked, m.index + m[0].length - 1);
    if (close < 0) continue;
    const after = /^\s*(?::\s*[^{;=]+)?\{/.exec(masked.slice(close, close + 200));
    if (after) add(m[1], m.index, close + after[0].length - 1);
  }
  // $effect(() => { ... }), onMount(...), and the like
  for (const cb of CALLBACKS) {
    const f4 = new RegExp(`(^|[^\\w$.])(${cb})\\s*\\(\\s*(?:async\\s*)?(?:\\([^)]*\\)|[A-Za-z_$][\\w$]*)\\s*=>\\s*\\{`, "g");
    while ((m = f4.exec(masked))) {
      const open = m.index + m[0].length - 1;
      add(m[2].replace(/\\/g, ""), m.index + m[1].length, open);
    }
  }
  // Number the blocks that share a name ($effect, onMount) so each can be listed on its own.
  found.sort((x, y) => x.line - y.line);
  const count = {};
  for (const f of found) {
    if (f.name.startsWith("$") || f.name === "onMount" || f.name === "onDestroy") {
      count[f.name] = (count[f.name] || 0) + 1;
      f.name = `${f.name}#${count[f.name]}`;
    }
  }
  return found;
}

function scanTs(src) {
  const masked = maskTs(src);
  const total = src.split("\n").length - (src.endsWith("\n") ? 1 : 0);
  return { total, code: total, fns: scanTsFunctions(masked), balanced: braceBalance(masked) };
}

function braceBalance(masked) {
  let d = 0;
  for (const c of masked) {
    if (c === "{") d++;
    else if (c === "}") d--;
  }
  return d === 0;
}

// ---------------------------------------------------------------- Svelte

function scanSvelte(src) {
  const total = src.split("\n").length - (src.endsWith("\n") ? 1 : 0);
  const lines = src.split("\n");
  const starts = lineStarts(src);
  let scriptLines = 0;
  let styleLines = 0;
  const fns = [];
  let balanced = true;
  const re = /<(script|style)\b[^>]*>([\s\S]*?)<\/\1>/g;
  let m;
  while ((m = re.exec(src))) {
    const bodyStart = m.index + m[0].indexOf(">") + 1;
    const first = lineOf(starts, bodyStart);
    const last = lineOf(starts, bodyStart + m[2].length);
    const count = last - first + 1;
    if (m[1] === "style") styleLines += count;
    else {
      scriptLines += count;
      const masked = maskTs(m[2]);
      if (!braceBalance(masked)) balanced = false;
      fns.push(...scanTsFunctions(masked, first - 1));
    }
  }
  const effects = fns.filter((f) => f.name.startsWith("$effect")).length;
  return {
    total,
    code: total,
    fns,
    balanced,
    parts: { script: scriptLines, style: styleLines, markup: Math.max(0, total - scriptLines - styleLines - 2 * (lines.filter((l) => /^<\/?(script|style)\b/.test(l)).length / 2)) },
    effects,
  };
}

// ---------------------------------------------------------------- tree walk

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    if (SKIP_DIRS.has(name)) continue;
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) yield* walk(p);
    else yield p;
  }
}

function kindOf(rel) {
  if (rel.startsWith("src-tauri/src/") && rel.endsWith(".rs")) return "rust";
  if (rel.startsWith("src/tests/")) return /\.(ts|svelte)$/.test(rel) ? "test" : null;
  if (rel.endsWith(".svelte")) return "svelte";
  if (rel.endsWith(".ts") || rel.endsWith(".js")) return "ts";
  if (rel.endsWith(".css")) return "css";
  return null;
}

export function measureTree() {
  const files = [];
  for (const root of ROOTS) {
    for (const p of walk(join(ROOT, root))) {
      const rel = relative(ROOT, p).split(sep).join("/");
      if (rel.endsWith(".d.ts")) continue;
      const kind = kindOf(rel);
      if (!kind) continue;
      const src = readFileSync(p, "utf8");
      let r;
      if (kind === "rust") r = scanRust(src);
      else if (kind === "svelte" || (kind === "test" && rel.endsWith(".svelte"))) r = scanSvelte(src);
      else if (kind === "css") r = { total: src.split("\n").length, code: src.split("\n").length, fns: [], balanced: true };
      else r = scanTs(src);
      files.push({ rel, kind, ...r });
    }
  }
  return files;
}

function fnKind(f) {
  if (f.kind === "rust") return "rust";
  if (f.kind === "svelte") return "svelte";
  return "ts";
}

// ---------------------------------------------------------------- allowlist and report

function loadAllowlist() {
  try {
    return JSON.parse(readFileSync(ALLOWLIST, "utf8"));
  } catch {
    return { files: {}, functions: {} };
  }
}

/** The largest function of each name in a file (`new`, `fmt` and friends repeat). */
function fnMaxima(f) {
  const out = {};
  for (const fn of f.fns) if (!out[fn.name] || fn.lines > out[fn.name].lines) out[fn.name] = fn;
  return out;
}

function allowed(limit, now) {
  const t = LIMITS.tolerance;
  return limit + Math.max(t.minLines, Math.ceil((limit * t.percent) / 100));
}

function buildAllowlist(files) {
  const out = { files: {}, functions: {} };
  for (const f of files) {
    const cap = LIMITS.files[f.kind].cap;
    if (f.code > cap) out.files[f.rel] = { lines: f.code };
    if (f.parts && f.kind === "svelte") {
      if (f.parts.script > LIMITS.svelteScript.cap) out.files[`${f.rel}#script`] = { lines: f.parts.script };
      if (f.effects > LIMITS.effects.cap) out.files[`${f.rel}#effects`] = { lines: f.effects };
    }
    if (f.kind === "test") continue;
    const fcap = LIMITS.functions[fnKind(f)].cap;
    for (const fn of Object.values(fnMaxima(f))) if (fn.lines > fcap) out.functions[`${f.rel}::${fn.name}`] = { lines: fn.lines };
  }
  const sorted = (o) => Object.fromEntries(Object.entries(o).sort(([a], [b]) => a.localeCompare(b)));
  return {
    _about: "Code that was over a cap when the limits were introduced. It must not grow past its recorded size plus the tolerance in scripts/check-size.mjs; new code goes in new modules. Regenerate deliberately with: node scripts/check-size.mjs --update-allowlist (after a split, so the entry shrinks or disappears).",
    files: sorted(out.files),
    functions: sorted(out.functions),
  };
}

/** One entry per line, so a diff of the allowlist reads at a glance. */
function formatAllowlist(list) {
  const block = (o) => "{\n" + Object.entries(o).map(([k, v]) => `    ${JSON.stringify(k)}: { "lines": ${v.lines} }`).join(",\n") + "\n  }";
  return `{\n  "_about": ${JSON.stringify(list._about)},\n  "files": ${block(list.files)},\n  "functions": ${block(list.functions)}\n}\n`;
}

function main() {
  const args = new Set(process.argv.slice(2));
  const files = measureTree();
  if (args.has("--update-allowlist")) {
    const list = buildAllowlist(files);
    writeFileSync(ALLOWLIST, formatAllowlist(list));
    console.log(`allowlist written: ${Object.keys(list.files).length} files, ${Object.keys(list.functions).length} functions`);
    return;
  }
  if (args.has("--stats")) return stats(files);

  const list = loadAllowlist();
  const violations = [];
  const soft = [];
  const stale = [];
  const unscanned = files.filter((f) => !f.balanced).map((f) => f.rel);

  for (const f of files) {
    const lim = LIMITS.files[f.kind];
    const rec = list.files[f.rel];
    if (f.code > lim.cap) {
      if (!rec) violations.push(`${f.rel}: ${f.code} lines, over the ${lim.cap} cap (${f.kind}); put new code in a new module`);
      else if (f.code > allowed(rec.lines))
        violations.push(`${f.rel}: grew from ${rec.lines} to ${f.code} lines (a grandfathered file may not grow); put new code in a new module`);
    } else if (rec) stale.push(`${f.rel}: now ${f.code} lines, under the cap; remove it from the allowlist (--update-allowlist)`);
    if (f.code > lim.target && f.code <= lim.cap) soft.push(`${f.rel}: ${f.code} lines (target ${lim.target})`);
    if (f.kind === "svelte") {
      const s = LIMITS.svelteScript;
      const srec = list.files[`${f.rel}#script`];
      if (f.parts.script > s.cap) {
        if (!srec) violations.push(`${f.rel}: script part is ${f.parts.script} lines, over the ${s.cap} cap; move logic into a module under src/lib`);
        else if (f.parts.script > allowed(srec.lines)) violations.push(`${f.rel}: script part grew from ${srec.lines} to ${f.parts.script} lines (grandfathered, may not grow)`);
      }
      const e = LIMITS.effects;
      const erec = list.files[`${f.rel}#effects`];
      if (f.effects > e.cap && f.effects > (erec ? erec.lines : 0))
        violations.push(`${f.rel}: ${f.effects} $effect blocks, over ${erec ? `its recorded ${erec.lines}` : `the cap of ${e.cap}`}; each effect is a place an error can take the page down`);
      if (f.parts.script > s.target && f.parts.script <= s.cap) soft.push(`${f.rel}: script part ${f.parts.script} lines (target ${s.target})`);
      if (f.effects > e.target && f.effects <= e.cap) soft.push(`${f.rel}: ${f.effects} $effect blocks (target ${e.target})`);
    }
    if (f.kind === "test") continue;
    const fl = LIMITS.functions[fnKind(f)];
    for (const fn of Object.values(fnMaxima(f))) {
      const key = `${f.rel}::${fn.name}`;
      const frec = list.functions[key];
      if (fn.lines > fl.cap) {
        if (!frec) violations.push(`${f.rel}:${fn.line} fn ${fn.name}: ${fn.lines} lines, over the ${fl.cap} cap; split it`);
        else if (fn.lines > allowed(frec.lines))
          violations.push(`${f.rel}:${fn.line} fn ${fn.name}: grew from ${frec.lines} to ${fn.lines} lines (grandfathered, may not grow)`);
      } else if (frec) stale.push(`${key}: now ${fn.lines} lines, under the cap; remove it from the allowlist`);
      if (fn.lines > fl.target && fn.lines <= fl.cap) soft.push(`${f.rel}:${fn.line} fn ${fn.name}: ${fn.lines} lines (target ${fl.target})`);
    }
  }

  const grandF = Object.keys(list.files).length;
  const grandN = Object.keys(list.functions).length;
  if (violations.length) {
    console.log(`Size check: ${violations.length} over the limits\n`);
    for (const v of violations) console.log("  " + v);
    console.log("\nThe limits and the reasons are in AGENTS.md (Size limits).");
  } else {
    console.log(`Size check: ok (${files.length} files; ${grandF} files and ${grandN} functions grandfathered)`);
  }
  if (stale.length) {
    console.log(`\n${stale.length} grandfathered entries shrank below the cap (tidy the allowlist when convenient):`);
    for (const s of stale) console.log("  " + s);
  }
  if (unscanned.length) console.log(`\nNote: braces did not balance in ${unscanned.join(", ")}; the function sizes there are unreliable.`);
  if (args.has("--all")) {
    console.log(`\nOver the soft target, under the cap (${soft.length}):`);
    for (const s of soft) console.log("  " + s);
  }
  process.exitCode = violations.length ? 1 : 0;
}

/** `--stats`: a distribution of the whole tree, for choosing limits. */
function stats(files) {
  const pct = (a, p) => a.length ? a[Math.min(a.length - 1, Math.floor((a.length * p) / 100))] : 0;
  const line = (label, vals) => {
    const a = [...vals].sort((x, y) => x - y);
    console.log(`${label.padEnd(16)} n=${String(a.length).padStart(4)} median=${pct(a, 50)} p75=${pct(a, 75)} p90=${pct(a, 90)} max=${a[a.length - 1] ?? 0}`);
  };
  for (const kind of Object.keys(LIMITS.files)) line(`files ${kind}`, files.filter((f) => f.kind === kind).map((f) => f.code));
  for (const k of ["rust", "ts", "svelte"]) line(`fns ${k}`, files.filter((f) => f.kind !== "test" && fnKind(f) === k).flatMap((f) => f.fns.map((x) => x.lines)));
  console.log("\ntop files:");
  for (const f of [...files].sort((a, b) => b.code - a.code).slice(0, 25)) console.log(`  ${String(f.code).padStart(5)} ${f.rel}${f.total !== f.code ? ` (total ${f.total})` : ""}${f.parts ? ` script ${f.parts.script} markup ${f.parts.markup} style ${f.parts.style} effects ${f.effects}` : ""}`);
  console.log("\ntop functions:");
  const all = files.filter((f) => f.kind !== "test").flatMap((f) => f.fns.map((x) => ({ ...x, rel: f.rel })));
  for (const f of all.sort((a, b) => b.lines - a.lines).slice(0, 30)) console.log(`  ${String(f.lines).padStart(5)} ${f.rel}:${f.line} ${f.name}`);
  const params = files.filter((f) => f.kind === "rust").flatMap((f) => f.fns.map((x) => ({ ...x, rel: f.rel })));
  console.log(`\nrust fns with >7 params: ${params.filter((p) => p.params > 7).length}; >5: ${params.filter((p) => p.params > 5).length} of ${params.length}`);
  const eff = files.filter((f) => f.effects !== undefined).map((f) => f.effects);
  line("effects/comp", eff);
  console.log("unbalanced:", files.filter((f) => !f.balanced).map((f) => f.rel).join(", ") || "none");
  const svs = files.filter((f) => f.kind === "svelte");
  line("svelte script", svs.map((f) => f.parts.script));
  line("svelte markup", svs.map((f) => f.parts.markup));
  line("svelte style", svs.map((f) => f.parts.style));
}

if (process.argv[1] && process.argv[1].endsWith("check-size.mjs")) main();

// Blind spots (an approximation, on purpose):
// - Rust: macro bodies (`macro_rules!`, the `cadence_commands!` table) are not functions; `fn` inside
//   a `#[cfg(test)]` block is skipped. Rust lines are counted without #[cfg(test)] blocks that start
//   at column 0.
// - TS/Svelte: handlers written inline in the markup (`onclick={() => { ... }}`) are not seen; nor
//   are anonymous callbacks passed to calls other than $effect, onMount and friends; an expression-
//   bodied arrow is measured to the end of its statement; regex literals are guessed.
// - Functions with the same name in one file (`new`, `fmt`) are reported as the longest of them.
// - Lines are physical lines, comments and blank lines included.

// What happened just before an error, for a bug report: the screens opened, the buttons pressed,
// the screen turning off and on, and what was logged. Kept in localStorage (small, last 60 steps)
// so it survives the reload that gets Cadence back after an error. "Report on GitHub" opens a new
// issue filled in with it; the user reads it and presses Submit on GitHub themselves.
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";

const REPO = "akbarharyadi/cadence";
const STEPS = "cadence-diag-steps";
const CRASH = "cadence-diag-crash";
const MAX_STEPS = 60;

type Step = { t: number; kind: string; text: string };
export type Crash = { at: number; message: string; stack?: string; where?: string; steps: Step[] };

let steps: Step[] = [];
let saveTimer: ReturnType<typeof setTimeout> | undefined;

function load<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

function clip(s: string, n: number) {
  s = s.replace(/\s+/g, " ").trim();
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

function text(v: unknown): string {
  if (v instanceof Error) return `${v.name}: ${v.message}`;
  if (typeof v === "string") return v;
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
}

/** Takes out what looks like a place on the disk (folder and file names say a lot about a
 *  person's music): "D:\Music\Artist\01 Song.flac" becomes "<path>.flac". */
// No lookbehind anywhere in here: WebKit before Safari 16.4 can't even parse one, and this file
// loads before anything can catch the error.
// Folders (names may have spaces; a name can't hold ":"), then the last part: either a file name
// (dots allowed, a few words at most) ending in an extension, which is kept, or, for a folder
// with no extension, up to four words, never the rest of the message.
const NAME = String.raw`[^\\/\r\n"'“”<>|:]`;
const WORD = String.raw`[^\\/\s"'“”<>|:;,()]`;
const REST =
  String.raw`(?:${NAME}*[\\/])*(?:${NAME}{0,120}?(\.[A-Za-z0-9]{2,5})(?![A-Za-z0-9]|\.[A-Za-z0-9])|${WORD}+(?:[ \t]+${WORD}+){0,3})`;
const WINDOWS_PATH = new RegExp(String.raw`(?:\b[A-Za-z]:[\\/]|\\\\[^\\\s]+\\)` + REST, "g");
// Any root: "/" + a folder + "/" + the rest. Not after a letter, digit, ":", "/" or "." (so not
// inside a URL or a date); the character before is captured and kept.
const UNIX_PATH = new RegExp(String.raw`(^|[^:/\w.\\])/[^\\/\s"'“”<>|:]+/` + REST, "g");

export function scrubPaths(s: string): string {
  return (
    s
      // Files shown through Tauri's asset protocol (the path is in the URL, encoded).
      .replace(/\b(?:https?:\/\/asset\.localhost|asset:\/\/localhost)\/[^\s"')]+/gi, "<file>")
      // file:///C:/… and Android's content://… URIs.
      .replace(/\b(?:file:\/\/\/?|content:\/\/)[^\s"')]+/gi, "<file>")
      // Windows: C:\…, C:/…, \\server\share\…
      .replace(WINDOWS_PATH, (_p, ext?: string) => `<path>${ext ?? ""}`)
      // Android, Linux, macOS: under any root.
      .replace(UNIX_PATH, (_p, pre: string, ext?: string) => `${pre}<path>${ext ?? ""}`)
  );
}

/** Words a button's label may keep in the log: what Cadence's own buttons say. Anything else (a
 *  song, an album, a folder's name) is left out. */
const UI_WORDS = new Set(
  `play pause next previous prev shuffle repeat back forward close cancel save done ok undo redo report reload retry try again update
  restart later open remove delete add new edit fix change choose find search settings sound listen now discover your stats albums
  album artists artist songs song favorites favorite library recently added needs attention health playlists playlist smart queue
  up lyrics mini player volume mute unmute rescan look check ignore keep allow access skip skipping stop rating rate sleep timer
  more menu scroll clear apply import sort filter show hide expand collapse select all yes no start radio mix mixes picture cover
  info tags copy copies move recycle bin go to from for on off the a an of and in with as this these that it everything restore
  yesterday today ask dj buy test key details sure use instead turn enable disable reset run`.split(/\s+/),
);

/** A clicked control, without what it's about: its `data-diag` name if it has one, else its role
 *  and the first words of its label that are Cadence's own ("button Remove …", not the folder). */
function tapLabel(el: Element): string {
  const named = el.closest("[data-diag]")?.getAttribute("data-diag");
  if (named) return clip(scrubPaths(named), 60);
  const role = el.getAttribute("role") || el.tagName.toLowerCase();
  const raw = (el.getAttribute("aria-label") || el.getAttribute("title") || el.textContent || "").replace(/\s+/g, " ").trim();
  const words = raw.split(" ").filter(Boolean);
  const kept: string[] = [];
  for (const w of words) {
    if (kept.length >= 4 || !UI_WORDS.has(w.toLowerCase().replace(/[…,.:!?]+$/, ""))) break;
    kept.push(w);
  }
  const label = kept.join(" ") + (kept.length < words.length ? (kept.length ? " …" : "…") : "");
  return label ? `${role} ${label}` : role;
}

/** Adds a step to the log. */
export function note(kind: string, what: string) {
  steps.push({ t: Date.now(), kind, text: clip(scrubPaths(what), 160) });
  if (steps.length > MAX_STEPS) steps.splice(0, steps.length - MAX_STEPS);
  // Written in one go a moment later: a burst of logging costs one write.
  clearTimeout(saveTimer);
  saveTimer = setTimeout(save, 250);
}

function save() {
  clearTimeout(saveTimer);
  try {
    localStorage.setItem(STEPS, JSON.stringify(steps));
  } catch {
    /* storage full: the log is only a nice-to-have */
  }
}

let installed = false;
/** Starts recording. Called once, as early as possible (hooks.client.ts). */
export function install() {
  if (installed || typeof window === "undefined") return;
  installed = true;
  steps = load<Step[]>(STEPS) ?? [];
  note("start", `Cadence opened (${location.pathname})`);

  for (const level of ["error", "warn"] as const) {
    const orig = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      note(level, args.map(text).join(" "));
      orig(...args);
    };
  }
  window.addEventListener("error", (e) => note("error", `${e.message} (${e.filename?.split("/").pop()}:${e.lineno})`));
  window.addEventListener("unhandledrejection", (e) => note("error", `Unhandled: ${text(e.reason)}`));
  document.addEventListener(
    "click",
    (e) => {
      const el = (e.target as HTMLElement | null)?.closest?.("button, a, [role=button], [role=tab], [role=option], [role=menuitem], [role=radio]");
      if (!el) return;
      note("tap", tapLabel(el));
    },
    true,
  );
  document.addEventListener("visibilitychange", () => {
    note("screen", document.visibilityState === "visible" ? "back on screen" : "hidden (screen off / app in background)");
    // Android may end the page while it's hidden: keep the log now.
    if (document.visibilityState === "hidden") save();
  });
  window.addEventListener("pagehide", save);
  window.addEventListener("pageshow", (e) => e.persisted && note("screen", "page restored"));
  window.addEventListener("online", () => note("net", "online"));
  window.addEventListener("offline", () => note("net", "offline"));
}

/** An error's message, also for the `{ message }` objects SvelteKit hands to error boundaries. */
export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message || error.name;
  if (error && typeof error === "object" && typeof (error as { message?: unknown }).message === "string") return (error as { message: string }).message;
  return text(error);
}

/** Remembers an error (with the steps before it) so it can still be reported after a reload. */
export function recordCrash(error: unknown, where?: string): Crash {
  const message = errorMessage(error);
  // SvelteKit runs handleError (which has the real Error, with its stack) before a boundary gets
  // the plain { message }: keep that stack.
  const earlier = load<Crash>(CRASH);
  const stack = error instanceof Error ? error.stack : earlier && earlier.message === message && Date.now() - earlier.at < 5000 ? earlier.stack : undefined;
  const crash: Crash = { at: Date.now(), message, stack, where, steps: [...steps] };
  note("crash", `${where ? `${where}: ` : ""}${crash.message}`);
  save(); // the error page reloads at once
  try {
    localStorage.setItem(CRASH, JSON.stringify(crash));
  } catch {
    /* ignore */
  }
  return crash;
}

/** The last error, if it happened in the last few minutes and hasn't been dealt with. */
export function recentCrash(withinMs = 5 * 60_000): Crash | null {
  const c = load<Crash>(CRASH);
  return c && Date.now() - c.at < withinMs ? c : null;
}

export function forgetCrash() {
  localStorage.removeItem(CRASH);
}

function time(t: number) {
  const d = new Date(t);
  return `${d.toTimeString().slice(0, 8)}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

/** Opens a new GitHub issue filled in with the error, the device and the steps before it. */
export async function reportCrash(crash: Crash) {
  let version = "?";
  try {
    version = await getVersion();
  } catch {
    /* ignore */
  }
  // (The steps were cleaned when they were logged; the error itself is cleaned here.)
  const message = scrubPaths(crash.message);
  const title = `[Error] ${clip(message, 90)}`;
  const head = [
    "**What happened:** _(what were you doing? e.g. turned the screen off and on again)_",
    "",
    `**Error:** ${message}${crash.where ? ` (in ${crash.where})` : ""}`,
    `**Cadence:** ${version}`,
    `**Device:** ${navigator.userAgent}`,
    `**Screen:** ${screen.width}×${screen.height} @${devicePixelRatio}x`,
    `**When:** ${new Date(crash.at).toString()}`,
    "",
  ];
  const stack = crash.stack ? ["<details><summary>Stack</summary>", "", "```", clip(scrubPaths(crash.stack), 1500).replace(/ at /g, "\n at "), "```", "</details>", ""] : [];
  // GitHub refuses very long URLs (~8 KB): drop the oldest steps until it fits.
  // Steps saved by an older version weren't cleaned: clean them again.
  let log = crash.steps.map((s) => `${time(s.t)} ${s.kind.padEnd(6)} ${scrubPaths(s.text)}`);
  let url = "";
  for (;;) {
    const body = [...head, ...stack, "<details open><summary>Steps before the error (newest last)</summary>", "", "```", ...log, "```", "</details>"].join("\n");
    url = `https://github.com/${REPO}/issues/new?title=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;
    if (url.length < 7500 || log.length === 0) break;
    log = log.slice(Math.max(1, Math.ceil(log.length / 8)));
  }
  await openUrl(url);
}

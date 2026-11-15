import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import { mockBackend } from "./helpers";

// The bug report is opened as a GitHub issue URL: what goes into it must hold no place on the disk
// and no song, album or folder name. The page's own code is the only thing exercised here; the
// "opener" command that would open the browser is captured by the mocked backend.
type Diag = typeof import("#lib/diag.js");

/** Reads back the text a report would put in the issue (the URL's body, decoded). */
async function reportText(diag: Diag, crash: ReturnType<Diag["recordCrash"]>) {
  const backend = mockBackend();
  await diag.reportCrash(crash);
  const url = backend.last("plugin:opener|open_url")?.url as string;
  expect(url, "the report should have been opened as a URL").toMatch(/^https:\/\/github\.com\//);
  const params = new URL(url).searchParams;
  return `${params.get("title")}\n${params.get("body")}`;
}

// Everything that would identify the person's files or music.
const LEAKS = [/someone/i, /D:\\/, /\bAlbums\b/, /Music/, /storage/i, /emulated/i, /a\.flac/, /Lemon/, /Yonezu/, /Bootleg/];
const expectClean = (text: string) => {
  for (const re of LEAKS) expect(text, `leaked ${re}`).not.toMatch(re);
};

let diag: Diag;
beforeEach(async () => {
  vi.resetModules(); // diag keeps its log in module state: every test starts a new one
  document.body.innerHTML = "";
  diag = await import("#lib/diag.js");
  diag.install();
});

describe("bug report redaction", () => {
  test("a click on a button labelled with a folder keeps the verb, not the folder", async () => {
    document.body.innerHTML = `<button>Remove D:\\Music\\Albums</button><button>Play Lemon by Kenshi Yonezu</button>`;
    const user = userEvent.setup();
    for (const b of document.querySelectorAll("button")) await user.click(b);

    const crash = diag.recordCrash(new Error("Something broke"), "page");
    const taps = crash.steps.filter((s) => s.kind === "tap").map((s) => s.text);
    expect(taps).toEqual(["button Remove …", "button Play …"]);

    expectClean(await reportText(diag, crash));
  });

  test("log lines with Windows and Android paths lose the path (and the song named in it)", async () => {
    console.warn("couldn't read C:\\Users\\someone\\Music\\Kenshi Yonezu\\Bootleg\\Lemon.flac to the end");
    console.error("open failed: /storage/emulated/0/Music/Kenshi Yonezu/Bootleg/Lemon.flac");
    console.warn("seen at C:\\Users\\someone\\Music\\a.flac and /storage/emulated/0/Music/a.flac");

    const crash = diag.recordCrash(new Error("Failed to read C:\\Users\\someone\\Music\\a.flac"), "page");
    const text = await reportText(diag, crash);

    expectClean(text);
    // It still says what happened.
    expect(text).toContain("<path>.flac");
    expect(text).toContain("couldn't read");
  });

  test("the error's own message and stack are cleaned when the report is made", async () => {
    const error = new Error("ENOENT: D:\\Music\\Albums\\Lemon.flac");
    error.stack = "Error: x\n at read (file:///C:/Users/someone/app.js:1:1)\n at D:\\Music\\Albums\\Lemon.flac:3:4";
    const crash = diag.recordCrash(error, "page");
    expectClean(await reportText(diag, crash));
  });

  test("steps saved by an older version (written unclean) are cleaned again at report time", async () => {
    const crash = diag.recordCrash(new Error("boom"), "page");
    crash.steps.push({ t: Date.now(), kind: "tap", text: "button D:\\Music\\Albums\\Lemon.flac" });
    expectClean(await reportText(diag, crash));
  });

  test("scrubPaths: Windows drive, UNC, Android, Linux and asset URLs", () => {
    expect(diag.scrubPaths("D:\\Music\\Albums\\01 Song.flac")).toBe("<path>.flac");
    expect(diag.scrubPaths("\\\\nas\\share\\Music\\Song.mp3 failed")).toBe("<path>.mp3 failed");
    expect(diag.scrubPaths("/storage/emulated/0/Music/a.flac")).toBe("<path>.flac");
    expect(diag.scrubPaths("/home/someone/Music/My Album/a.flac")).toBe("<path>.flac");
    expect(diag.scrubPaths("http://asset.localhost/D%3A%5CMusic%5Ca.flac")).toBe("<file>");
    expect(diag.scrubPaths("nothing to hide here")).toBe("nothing to hide here");
  });
});

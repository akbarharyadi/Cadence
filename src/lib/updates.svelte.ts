// Keeping Cadence up to date.
//
// Desktop: the Tauri updater finds a signed new version on GitHub Releases (latest.json), downloads
// it in the background, then installs it when the user restarts.
// Android: the updater plugin doesn't run on phones, so Cadence reads the latest GitHub release,
// downloads the APK and hands it to Android's installer (which asks the user to confirm).

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const RELEASES = "https://api.github.com/repos/akbarharyadi/cadence/releases/latest";
const EVERY = 6 * 60 * 60 * 1000;
const isAndroid = typeof navigator !== "undefined" && navigator.userAgent.includes("Android");

type Phase = "idle" | "checking" | "current" | "downloading" | "ready" | "available" | "error";
type DesktopUpdate = import("@tauri-apps/plugin-updater").Update;

/** "0.2.10" > "0.2.9". */
function newer(a: string, b: string): boolean {
  const pa = a.replace(/^v/, "").split(/[.-]/).map((n) => parseInt(n) || 0);
  const pb = b.replace(/^v/, "").split(/[.-]/).map((n) => parseInt(n) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    if ((pa[i] ?? 0) !== (pb[i] ?? 0)) return (pa[i] ?? 0) > (pb[i] ?? 0);
  }
  return false;
}

class Updates {
  version = $state("");
  phase = $state<Phase>("idle");
  /** The version on offer. */
  next = $state("");
  progress = $state(0);
  message = $state("");
  /** The banner was closed for this version. */
  dismissed = $state(false);
  auto = $state(typeof localStorage === "undefined" || localStorage.getItem("cadence-auto-update") !== "off");

  /** Desktop: the installer is running (the Restart button waits). */
  installing = $state(false);
  /** Desktop: the update is installed, only the restart is left. */
  private installed = false;
  private desktop: DesktopUpdate | null = null;
  private apkUrl = "";
  private timer: ReturnType<typeof setInterval> | undefined;

  /** Starts the automatic checks (main window only). */
  async start() {
    this.version = await getVersion();
    if (this.auto) setTimeout(() => this.check(true), 15_000);
    this.timer = setInterval(() => this.auto && this.check(true), EVERY);
  }

  setAuto(on: boolean) {
    this.auto = on;
    localStorage.setItem("cadence-auto-update", on ? "on" : "off");
  }

  /** Looks for a new version; in the background (quiet) problems aren't shown. */
  async check(quiet = false) {
    if (this.phase === "checking" || this.phase === "downloading" || this.phase === "ready") return;
    this.phase = "checking";
    this.message = "";
    try {
      if (isAndroid) await this.checkAndroid();
      else await this.checkDesktop();
    } catch (e) {
      this.phase = quiet ? "idle" : "error";
      // A release whose build for this computer isn't uploaded yet (older releases were published
      // while still building): not a real problem, it'll be there on a later check.
      this.message = /fallback platforms|platforms` object/.test(String(e))
        ? "A new version is still being prepared for this computer. Try again in a few minutes."
        : `Couldn't check for updates: ${e}`;
    }
  }

  private async checkDesktop() {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (!update) {
      this.phase = "current";
      return;
    }
    this.desktop = update;
    this.next = update.version;
    this.dismissed = false;
    // Download now, quietly; install when the user restarts.
    this.phase = "downloading";
    let total = 0;
    let got = 0;
    await update.download((e) => {
      if (e.event === "Started") total = e.data.contentLength ?? 0;
      else if (e.event === "Progress") {
        got += e.data.chunkLength;
        if (total) this.progress = got / total;
      }
    });
    this.phase = "ready";
  }

  private async checkAndroid() {
    const r = await fetch(RELEASES, { headers: { Accept: "application/vnd.github+json" } });
    if (!r.ok) throw new Error(`GitHub answered ${r.status}`);
    const rel = (await r.json()) as { tag_name: string; assets: { name: string; browser_download_url: string }[] };
    const apk = rel.assets.find((a) => a.name.endsWith(".apk"));
    if (!apk || !newer(rel.tag_name, this.version)) {
      this.phase = "current";
      return;
    }
    this.next = rel.tag_name.replace(/^v/, "");
    this.apkUrl = apk.browser_download_url;
    this.dismissed = false;
    this.phase = "available";
  }

  /** Desktop: install the downloaded version and restart. Android: download and open the installer. */
  async apply() {
    if (!isAndroid) {
      if (!this.desktop || this.installing) return;
      this.installing = true;
      this.message = "";
      try {
        const { relaunch } = await import("@tauri-apps/plugin-process");
        // Installed already by an earlier try whose restart failed: only restart.
        if (!this.installed) {
          await this.desktop.install();
          this.installed = true;
        }
        await relaunch();
      } catch (e) {
        // Still "ready": the downloaded update is kept, so the banner (and Settings) offer to try
        // again. Seen: "Failed to kill Cadence" when Windows' installer couldn't close Cadence.
        const why = String(e).replace(/^Error:\s*/, "");
        this.message = this.installed
          ? `The update is installed but Cadence couldn't restart (${why}). Close Cadence and open it again.`
          : /kill|in use|access is denied|being used/i.test(why)
            ? `The update couldn't be installed: Windows couldn't close Cadence (${why}). Try again, or close Cadence and open it again.`
            : `The update couldn't be installed: ${why}. Try again.`;
        this.phase = "ready";
        this.dismissed = false;
        console.error("[Cadence] update failed:", e);
      } finally {
        this.installing = false;
      }
      return;
    }
    this.phase = "downloading";
    this.progress = 0;
    const off = await listen<number>("update-progress", (e) => (this.progress = e.payload));
    try {
      const path = await invoke<string>("download_update_apk", { url: this.apkUrl });
      const android = (window as unknown as { CadenceAndroid?: { installApk(p: string): string } }).CadenceAndroid;
      const answer = android?.installApk(path);
      this.phase = "available";
      this.message =
        answer === "permission"
          ? "Allow Cadence to install apps in the screen that opened, then tap Update again."
          : "Android's installer is open: tap Update there.";
    } catch (e) {
      this.phase = "error";
      this.message = `The update didn't download: ${e}`;
    } finally {
      off();
    }
  }
}

export const updates = new Updates();

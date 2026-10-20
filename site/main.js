// Cadence landing page: the breathing wave, and download links for the visitor's system.

// ---------- the wave ----------
const path = document.getElementById("wave-path");
const still = matchMedia("(prefers-reduced-motion: reduce)").matches;

function draw(t) {
  // A soft sum of two sines, like the logo's curve stretched across the page.
  const w = 1200;
  const points = [];
  for (let x = 0; x <= w; x += 12) {
    const y = 110 + Math.sin(x / 95 + t) * 58 * Math.sin(x / 600 + t * 0.35 + 0.6) + Math.sin(x / 41 - t * 1.4) * 7;
    points.push(`${x},${y.toFixed(1)}`);
  }
  path.setAttribute("d", "M" + points.join(" L"));
}

if (path) {
  if (still) draw(0.8);
  else {
    const start = performance.now();
    const tick = (now) => {
      draw((now - start) / 2600);
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }
}

// ---------- downloads ----------
const ua = navigator.userAgent;
const os = /Windows/.test(ua) ? "windows" : /Mac OS X|Macintosh/.test(ua) ? "mac" : /Linux|X11/.test(ua) && !/Android/.test(ua) ? "linux" : null;
const names = { windows: "Windows", mac: "macOS", linux: "Linux" };

/** Picks the right file from a GitHub release for each system. */
function assetFor(assets, system) {
  const find = (re) => assets.find((a) => re.test(a.name));
  if (system === "windows") return find(/x64-setup\.exe$/i) || find(/\.msi$/i);
  if (system === "mac") return find(/universal\.dmg$/i) || find(/\.dmg$/i);
  if (system === "linux") return find(/\.AppImage$/i) || find(/\.deb$/i);
}

async function loadRelease() {
  try {
    const r = await fetch("https://api.github.com/repos/akbarharyadi/cadence/releases/latest");
    if (!r.ok) return;
    const rel = await r.json();
    const label = document.getElementById("release-name");
    if (label) label.textContent = `${rel.name || rel.tag_name}, released ${new Date(rel.published_at).toLocaleDateString()}`;
    for (const el of document.querySelectorAll(".platform")) {
      const a = assetFor(rel.assets || [], el.dataset.os);
      if (a) el.href = a.browser_download_url;
    }
    const main = document.getElementById("download-main");
    const mine = os && assetFor(rel.assets || [], os);
    if (main && mine) main.href = mine.browser_download_url;
  } catch {
    // Offline or rate-limited: the links still point at the releases page.
  }
}

if (os) {
  const main = document.getElementById("download-main");
  if (main) main.textContent = `Download for ${names[os]}`;
  document.querySelector(`.platform[data-os="${os}"]`)?.classList.add("mine");
}
loadRelease();

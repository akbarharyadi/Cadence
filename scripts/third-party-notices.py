#!/usr/bin/env python3
"""Writes static/third-party-notices.txt: the license notices of everything Cadence is built from.

MIT, Apache, BSD and similar licenses ask that their notices travel with the program, so the file
ships inside the app (Settings > About > Open-source licenses) and is regenerated for each release.

It lists every Rust crate in Cargo.lock (all platforms) with its license, then the license texts,
each identical text printed once with the crates that use it. Then the web packages and fonts
bundled into the interface.

Usage: python scripts/third-party-notices.py   (needs `cargo` and the crates' sources; run
`cargo fetch` first on a fresh machine)
"""

import hashlib
import json
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "static" / "third-party-notices.txt"
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|copyright|unlicense)([-._].*)?$", re.I)
# Web packages whose code or files end up in the built interface.
WEB = [
    "svelte",
    "@sveltejs/kit",
    "@tauri-apps/api",
    "@tauri-apps/plugin-dialog",
    "@tauri-apps/plugin-opener",
    "@tauri-apps/plugin-process",
    "@tauri-apps/plugin-updater",
    "@fontsource-variable/bricolage-grotesque",
    "@fontsource-variable/instrument-sans",
]


def license_texts(folder: pathlib.Path) -> list[tuple[str, str]]:
    found = []
    if not folder.is_dir():
        return found
    for f in sorted(folder.iterdir()):
        if f.is_file() and LICENSE_FILE.match(f.name) and f.stat().st_size < 200_000:
            text = f.read_text(encoding="utf-8", errors="replace").strip()
            if text:
                found.append((f.name, text))
    return found


def main() -> None:
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=ROOT / "src-tauri",
            capture_output=True,
            check=True,
            encoding="utf-8",
        ).stdout
    )
    own = {p["id"] for p in meta["packages"] if p["source"] is None}
    crates = sorted((p for p in meta["packages"] if p["id"] not in own), key=lambda p: (p["name"].lower(), p["version"]))

    # Identical texts are printed once, with every crate that uses them.
    texts: dict[str, dict] = {}
    missing = []
    for p in crates:
        name = f'{p["name"]} {p["version"]}'
        files = license_texts(pathlib.Path(p["manifest_path"]).parent)
        if not files:
            missing.append((name, p.get("license") or "see crate"))
        for fname, text in files:
            # The same license often differs only in spacing or line breaks.
            key = hashlib.sha1(" ".join(text.lower().split()).encode()).hexdigest()
            entry = texts.setdefault(key, {"text": text, "file": fname, "users": []})
            entry["users"].append(name)

    web = []
    for pkg in WEB:
        folder = ROOT / "node_modules" / pkg
        try:
            info = json.loads((folder / "package.json").read_text(encoding="utf-8"))
        except FileNotFoundError:
            continue
        web.append((f'{pkg} {info.get("version", "")}', info.get("license", ""), license_texts(folder)))

    lines = [
        "Cadence: open-source licenses",
        "",
        "Cadence is free software under the GNU General Public License v3.0 (see LICENSE).",
        "It is built from the open-source projects below. Their licenses and notices follow.",
        "",
        f"Rust crates ({len(crates)})",
        "=" * 60,
    ]
    lines += [f'{p["name"]} {p["version"]}  ({p.get("license") or "see below"})' for p in crates]
    lines += ["", f"Web packages and fonts ({len(web)})", "=" * 60]
    lines += [f"{name}  ({lic})" for name, lic, _ in web]
    lines += ["", "", "License texts", "=" * 60]
    for entry in sorted(texts.values(), key=lambda e: e["users"][0].lower()):
        lines += ["", "-" * 60, "Used by: " + ", ".join(entry["users"]), f'({entry["file"]})', "-" * 60, entry["text"]]
    for name, lic, files in web:
        for fname, text in files:
            lines += ["", "-" * 60, f"Used by: {name}", f"({fname})", "-" * 60, text]
    if missing:
        lines += ["", "-" * 60, "These crates ship no license file; their license is as stated in Cargo.toml:", "-" * 60]
        lines += [f"{name}: {lic}" for name, lic in missing]
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"{OUT.relative_to(ROOT)}: {len(crates)} crates, {len(web)} web packages, {len(texts)} distinct texts, {len(missing)} without a file")


if __name__ == "__main__":
    main()

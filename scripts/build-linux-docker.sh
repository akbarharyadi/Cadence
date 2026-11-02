#!/usr/bin/env bash
# Builds Cadence's Linux packages (.deb and .AppImage) in Docker, the same way CI does on Ubuntu
# 22.04, from any machine with Docker (Windows with Git Bash, macOS, Linux).
#
#   ./scripts/build-linux-docker.sh             # packages land in dist/linux/
#   ./scripts/build-linux-docker.sh <out-dir>   # or somewhere else
#
# Builds the working tree as it is, uncommitted changes included: git's file list (so no
# node_modules, target or other ignored files) is streamed into the container, and nothing in the
# repository is written. Named volumes keep the Rust registry, the target folder and the pnpm store
# between runs (docker volume rm cadence-linux-cargo cadence-linux-target cadence-linux-cache to
# start over): the first build takes ~20 minutes, later ones a few.
# With TAURI_SIGNING_PRIVATE_KEY set, the AppImage is signed for the updater (a .sig next to it).
set -euo pipefail
# Git Bash: pass container paths to docker as they are, and host paths as C:/... (pwd -W).
export MSYS_NO_PATHCONV=1
abs() { (cd "$1" && (pwd -W 2>/dev/null || pwd)); }

root="$(abs "$(dirname "$0")/..")"
mkdir -p "${1:-$root/dist/linux}"
out="$(abs "${1:-$root/dist/linux}")"
image=cadence-linux-build

echo "== build image ($image)"
docker build -t "$image" -f "$root/scripts/docker/linux.Dockerfile" "$root/scripts/docker"

echo "== Cadence $(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$root/src-tauri/tauri.conf.json") for Linux"
cd "$root"
git ls-files -z --cached --others --exclude-standard |
  tar -c --null --ignore-failed-read -T - -f - |
  docker run -i --rm \
    -v cadence-linux-cargo:/usr/local/cargo/registry \
    -v cadence-linux-target:/target \
    -v cadence-linux-cache:/cache \
    --mount "type=bind,src=$out,dst=/out" \
    -e CARGO_TARGET_DIR=/target -e XDG_CACHE_HOME=/cache \
    -e TAURI_SIGNING_PRIVATE_KEY -e TAURI_SIGNING_PRIVATE_KEY_PASSWORD \
    -e HOST_ID="$(id -u):$(id -g)" \
    "$image" bash -euo pipefail -c '
      tar -x -C /work
      pnpm install --frozen-lockfile --store-dir /cache/pnpm
      # Open-source license notices shipped in the app, as CI writes them.
      cargo fetch --locked --manifest-path src-tauri/Cargo.toml
      python3 scripts/third-party-notices.py
      # No update signature without the signing key (it is a CI secret).
      config=()
      [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]] || config=(--config "{\"bundle\":{\"createUpdaterArtifacts\":false}}")
      rm -rf /target/release/bundle
      pnpm tauri build --ci --bundles deb,appimage "${config[@]}"
      cd /target/release/bundle
      cp deb/*.deb appimage/*.AppImage* /out/
      chown "$HOST_ID" /out/* 2>/dev/null || true'

ls -l "$out"

#!/usr/bin/env bash
# Builds Cadence for macOS (Apple Silicon and Intel in one app) on a Mac, the same way CI does.
#
#   ./scripts/build-macos.sh            # on the Mac, in the repository
#   ./scripts/build-macos.sh --remote <user@host>   # from another machine: copies the working tree
#                                                   # (no node_modules/target) and builds over SSH
#
# Needs on the Mac: Xcode command line tools, Rust (rustup), Node 22 and pnpm 10.
# The result: src-tauri/target/universal-apple-darwin/release/bundle/{dmg,macos}/
set -euo pipefail

if [[ "${1:-}" == "--remote" ]]; then
  host="${2:?usage: build-macos.sh --remote user@host}"
  root="$(cd "$(dirname "$0")/.." && pwd)"
  echo "== copying the working tree to $host:~/cadence-build"
  rsync -az --delete \
    --exclude node_modules --exclude target --exclude .svelte-kit --exclude build \
    --exclude 'native/windows/*/bin' --exclude 'native/windows/*/obj' --exclude 'native/windows/*/publish' \
    "$root/" "$host:cadence-build/"
  ssh "$host" 'cd ~/cadence-build && bash scripts/build-macos.sh'
  echo "== fetching the .dmg"
  mkdir -p "$root/dist/macos"
  scp "$host:cadence-build/src-tauri/target/universal-apple-darwin/release/bundle/dmg/*.dmg" "$root/dist/macos/"
  exit 0
fi

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
rustup target add aarch64-apple-darwin x86_64-apple-darwin
pnpm install --frozen-lockfile
# No update signature without the signing key (it's a CI secret).
config=()
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  config=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
fi
pnpm tauri build --target universal-apple-darwin "${config[@]}"
ls -lh src-tauri/target/universal-apple-darwin/release/bundle/dmg/

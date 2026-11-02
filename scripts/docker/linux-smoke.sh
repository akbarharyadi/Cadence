#!/usr/bin/env bash
# Smoke-tests the .deb from build-linux-docker.sh: installs it in a clean Ubuntu 22.04 container,
# starts Cadence on a virtual display (Xvfb) with an empty home, and after ~20 s saves a screenshot
# (smoke.png) and what it printed (smoke.log) next to the package. Never sees your own library.
#
#   ./scripts/docker/linux-smoke.sh [dist-dir]    # default dist/linux
set -euo pipefail
# Git Bash: pass container paths to docker as they are, and host paths as C:/... (pwd -W).
export MSYS_NO_PATHCONV=1
abs() { (cd "$1" && (pwd -W 2>/dev/null || pwd)); }

dist="$(abs "${1:-$(dirname "$0")/../../dist/linux}")"

docker run --rm --mount "type=bind,src=$dist,dst=/dist" ubuntu:22.04 bash -euo pipefail -c '
  export DEBIAN_FRONTEND=noninteractive
  echo "== installing $(ls /dist/*.deb) and Xvfb"
  # ca-certificates: every desktop has them, and the online lookups need them.
  apt-get update -qq
  apt-get install -y -qq ca-certificates xvfb dbus-x11 imagemagick /dist/*.deb >/dev/null
  # WebKit falls back to software rendering by itself. (WEBKIT_DISABLE_COMPOSITING_MODE=1 is no
  # help here: the window then stops repainting after the first frame.)
  export HOME=$(mktemp -d) DISPLAY=:99
  Xvfb :99 -screen 0 1440x900x24 -nolisten tcp & sleep 2
  dbus-run-session -- cadence >/dist/smoke.log 2>&1 & app=$!
  sleep 20
  import -window root /dist/smoke.png
  if kill -0 $app 2>/dev/null; then echo "== running after 20 s: smoke.png, smoke.log"
  else echo "== Cadence exited early:"; cat /dist/smoke.log; exit 1; fi'

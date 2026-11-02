# Cadence's Linux build environment, used by scripts/build-linux-docker.sh: the same Ubuntu 22.04
# and packages as CI (release.yml), so the .deb and AppImage run where the released ones do
# (glibc 2.35 and newer). The sources aren't in the image; the script streams them in per build.
FROM node:22-bookworm-slim AS node

FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential cmake curl ca-certificates file git python3 xz-utils \
      libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
      libasound2-dev libdbus-1-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Node 22 (the official build, glibc 2.28+) and pnpm 10.
COPY --from=node /usr/local/ /usr/local/
RUN npm install -g pnpm@10

# Rust stable. The registry and the target folder are named volumes (see the script).
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable

# The AppImage tools Tauri downloads are AppImages themselves, and a container has no FUSE.
ENV APPIMAGE_EXTRACT_AND_RUN=1
WORKDIR /work

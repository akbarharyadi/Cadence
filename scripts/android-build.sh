#!/usr/bin/env bash
# Builds the Android app on Windows without Developer Mode.
#
# `tauri android build` compiles the Rust library and then *symlinks* it into the Android project,
# which Windows only allows in Developer Mode. This runs the same steps, but copies (and strips)
# the library instead, then packages the APK with Gradle.
#
# Usage: scripts/android-build.sh [debug|release]   (default: debug; release needs signing)
set -euo pipefail
profile="${1:-debug}"
root="$(cd "$(dirname "$0")/.." && pwd)"
: "${ANDROID_HOME:=C:/Android/android-sdk}"
: "${NDK_HOME:=$ANDROID_HOME/ndk/28.2.13676358}"
export ANDROID_HOME NDK_HOME

cd "$root"
flag=""
[ "$profile" = "debug" ] && flag="--debug"
# Compiles everything; only the final symlink step fails without Developer Mode.
pnpm tauri android build $flag --apk --target aarch64 || true

so="src-tauri/target/aarch64-linux-android/$profile/libcadence_lib.so"
[ -f "$so" ] || { echo "Rust library wasn't built: $so"; exit 1; }
jni="src-tauri/gen/android/app/src/main/jniLibs/arm64-v8a"
mkdir -p "$jni"
rm -f "$jni/libcadence_lib.so"
"$NDK_HOME/toolchains/llvm/prebuilt/windows-x86_64/bin/llvm-strip.exe" -o "$jni/libcadence_lib.so" "$so"

cd src-tauri/gen/android
task="assembleArm64$(tr '[:lower:]' '[:upper:]' <<< "${profile:0:1}")${profile:1}"
./gradlew.bat "$task" -x "rustBuildArm64$(tr '[:lower:]' '[:upper:]' <<< "${profile:0:1}")${profile:1}" --console=plain
ls -la app/build/outputs/apk/arm64/"$profile"/*.apk

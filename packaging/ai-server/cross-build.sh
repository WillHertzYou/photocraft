#!/usr/bin/env bash
# Builds photocraft.exe for Windows on Linux, the way the `ai` branch is built while GitHub
# Actions is off for this fork: a release build for x86_64-pc-windows-gnu (MinGW), stripped,
# with its SHA-256 next to it. Install the result with install-photocraft.ps1's steps by hand
# (copy it over D:\AI\PhotoCraft\photocraft.exe with PhotoCraft closed).
#
# Needs rustup and MinGW (Debian/Ubuntu: apt-get install gcc-mingw-w64-x86-64). The craft-fonts
# checkout is optional, as in windows-ai-build.yml; without it the build uses fallback fonts.
#   packaging/ai-server/cross-build.sh [out-dir]
set -euo pipefail
cd "$(dirname "$0")/../.."
out="${1:-target/windows-ai}"
fonts="${CRAFT_FONTS_DIR:-../craft-fonts}"

rustup target add x86_64-pc-windows-gnu >/dev/null
if [ ! -d "$fonts" ]; then
    git clone -q https://github.com/storytold/craft-fonts "$fonts" 2>/dev/null &&
        git -C "$fonts" checkout -q abb83316d96aa59c1cf64784289e378fe9fa5695 || true
fi
if [ -d "$fonts" ]; then export CRAFT_FONTS_DIR="$(cd "$fonts" && pwd)"; fi

cargo build --release --locked -p photocraft --target x86_64-pc-windows-gnu
mkdir -p "$out"
x86_64-w64-mingw32-strip -o "$out/photocraft.exe" target/x86_64-pc-windows-gnu/release/photocraft.exe
(cd "$out" && sha256sum photocraft.exe > photocraft.exe.sha256 && cat photocraft.exe.sha256)

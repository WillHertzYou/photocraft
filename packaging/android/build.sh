#!/usr/bin/env bash
# Build the installable Android (PWA) site from a PhotoCraft web release.
#
# Downloads photocraft-web-<version>.zip from the upstream GitHub release, verifies it against
# SHA256SUMS.txt, then adds a web app manifest, home-screen icons and the page tags Chrome on
# Android needs to install PhotoCraft as an app. The app itself is unchanged.
#
#   packaging/android/build.sh [version] [out-dir]     # defaults: 0.7.0, dist/android
#   packaging/android/build.sh local [out-dir]         # use dist/web from `trunk build --release`
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
VERSION="${1:-0.7.0}"
OUT="${2:-$ROOT/dist/android}"
REPO="${PHOTOCRAFT_UPSTREAM:-storytold/photocraft}"

rm -rf "$OUT"
mkdir -p "$OUT"
if [ "$VERSION" = local ]; then
  [ -f "$ROOT/dist/web/index.html" ] || { echo "error: run 'trunk build --release' in apps/photocraft-web first" >&2; exit 1; }
  cp -R "$ROOT/dist/web/." "$OUT/"
else
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  BASE="https://github.com/$REPO/releases/download/v$VERSION"
  ZIP="photocraft-web-$VERSION.zip"
  curl -fsSL -o "$TMP/$ZIP" "$BASE/$ZIP"
  curl -fsSL -o "$TMP/SHA256SUMS.txt" "$BASE/SHA256SUMS.txt"
  (cd "$TMP" && grep " $ZIP\$" SHA256SUMS.txt | sha256sum -c -)
  unzip -q "$TMP/$ZIP" -d "$TMP/unpacked"
  cp -R "$TMP/unpacked/photocraft-web-$VERSION/." "$OUT/"
fi

cp "$HERE/manifest.webmanifest" "$OUT/"
mkdir -p "$OUT/icons"
cp "$HERE"/icons/*.png "$OUT/icons/"
cp "$HERE/apple-touch-icon.png" "$OUT/"

# Page tags: manifest link, theme colour, icons. Inserted once, before </head>.
TAGS='  <link rel="manifest" href="manifest.webmanifest">\n  <meta name="theme-color" content="#262626">\n  <meta name="mobile-web-app-capable" content="yes">\n  <link rel="icon" type="image/png" sizes="192x192" href="icons/icon-192.png">\n  <link rel="apple-touch-icon" href="apple-touch-icon.png">'
grep -q 'rel="manifest"' "$OUT/index.html" || sed -i.bak "s|</head>|$TAGS\n</head>|" "$OUT/index.html"
rm -f "$OUT/index.html.bak"
# Let the page reach the screen edges (notch/cut-out) when installed full screen.
sed -i.bak 's#initial-scale=1, user-scalable=no"#initial-scale=1, user-scalable=no, viewport-fit=cover"#' "$OUT/index.html"
rm -f "$OUT/index.html.bak"

# Netlify / Cloudflare Pages headers for the added files.
if ! grep -q webmanifest "$OUT/_headers" 2>/dev/null; then
  printf '\n/manifest.webmanifest\n  Content-Type: application/manifest+json\n  Cache-Control: no-cache\n' >>"$OUT/_headers"
fi

# Netlify: the site is already built, so publish the folder as is.
printf '[build]\n  publish = "."\n  command = ""\n' >"$OUT/netlify.toml"

grep -q 'rel="manifest"' "$OUT/index.html" || { echo "error: could not add the manifest link to index.html" >&2; exit 1; }
echo "Android PWA site ready in $OUT"

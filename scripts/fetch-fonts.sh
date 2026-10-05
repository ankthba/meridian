#!/bin/bash
# Fetches the Iosevka Fixed (SS08 stylistic set) TTFs into the app bundle
# resources. Iosevka is SIL OFL 1.1 with no Reserved Font Name; the license
# file is copied alongside. Fonts are not committed (size); this script is
# idempotent and run by scripts/build-app.sh.
set -euo pipefail
VERSION="34.9.0"
PKG="PkgTTF-IosevkaFixedSS08-${VERSION}.zip"
URL="https://github.com/be5invis/Iosevka/releases/download/v${VERSION}/${PKG}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/app/Meridian/Resources/Fonts"
CACHE="${MERIDIAN_CACHE:-$HOME/Library/Caches/Meridian-build}"
mkdir -p "$DEST" "$CACHE"
if ls "$DEST"/IosevkaFixedSS08-Regular.ttf >/dev/null 2>&1; then
  echo "fonts present"; exit 0
fi
if [ ! -f "$CACHE/$PKG" ]; then
  echo "downloading $URL"
  curl -fL --retry 3 -o "$CACHE/$PKG.part" "$URL"
  mv "$CACHE/$PKG.part" "$CACHE/$PKG"
fi
TMP="$(mktemp -d)"
unzip -q -o "$CACHE/$PKG" -d "$TMP"
for w in Regular Medium Bold; do
  f="$(find "$TMP" -name "IosevkaFixedSS08-${w}.ttf" | head -1)"
  [ -n "$f" ] && cp "$f" "$DEST/"
done
curl -fsSL "https://raw.githubusercontent.com/be5invis/Iosevka/v${VERSION}/LICENSE.md" -o "$DEST/Iosevka-LICENSE.md" || true
rm -rf "$TMP"
ls -la "$DEST"

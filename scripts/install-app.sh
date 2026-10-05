#!/bin/bash
# Installs the release build to ~/Applications/Meridian.app (a stable path
# for the Dock and Spotlight) and relaunches it if it was running.
#   scripts/install-app.sh           # install the existing release build
#   scripts/install-app.sh --build   # build first (scripts/build-app.sh release)
#   scripts/install-app.sh --dock    # also pin to the Dock if not already there
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/app/build/DerivedData/Build/Products/Release/Meridian.app"
DEST="$HOME/Applications/Meridian.app"
BUILD=0; DOCK=0
for a in "$@"; do
  case "$a" in
    --build) BUILD=1 ;;
    --dock) DOCK=1 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done
[ "$BUILD" = 1 ] && "$ROOT/scripts/build-app.sh" release
[ -d "$SRC" ] || { echo "no release build at $SRC (run with --build)" >&2; exit 1; }

was_running=0
if pgrep -x Meridian >/dev/null; then
  was_running=1
  osascript -e 'tell application id "dev.meridian.Meridian" to quit' >/dev/null 2>&1 || true
  for _ in $(seq 1 50); do pgrep -x Meridian >/dev/null || break; sleep 0.1; done
fi

mkdir -p "$HOME/Applications"
rm -rf "$DEST"
ditto "$SRC" "$DEST"
touch "$DEST"   # refresh the Finder/Dock icon cache
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST"
echo "installed → $DEST"

if [ "$DOCK" = 1 ]; then
  if defaults read com.apple.dock persistent-apps 2>/dev/null | grep -q "Applications/Meridian.app"; then
    echo "already in the Dock"
  else
    defaults write com.apple.dock persistent-apps -array-add \
      "<dict><key>tile-data</key><dict><key>file-data</key><dict><key>_CFURLString</key><string>file://$DEST/</string><key>_CFURLStringType</key><integer>15</integer></dict></dict></dict>"
    killall Dock
    echo "pinned to the Dock"
  fi
fi

[ "$was_running" = 1 ] && open "$DEST"
exit 0

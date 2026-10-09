#!/bin/bash
# Full build: fonts, Rust core (+bindings), third-party notices, Xcode project, app.
#   scripts/build-app.sh [debug|release]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${1:-debug}"
CONFIG=$([ "$PROFILE" = "release" ] && echo Release || echo Debug)
"$ROOT/scripts/build-core.sh" "$PROFILE" >/dev/null
# After the core build, so Cargo.lock is current; fails on a non-permissive license.
"$ROOT/scripts/third-party-notices.sh" >/dev/null
cd "$ROOT/app"
xcodegen generate --spec project.yml >/dev/null
xcodebuild -project Meridian.xcodeproj -scheme Meridian -configuration "$CONFIG" -destination 'platform=macOS,arch=arm64' \
  -derivedDataPath build/DerivedData build 2>&1 | grep -E "error:|BUILD (SUCCEEDED|FAILED)" | sort -u
APP="$ROOT/app/build/DerivedData/Build/Products/$CONFIG/Meridian.app"
# Keep build products out of Spotlight and LaunchServices, so opening
# "Meridian" always launches the installed copy (scripts/install-app.sh),
# never a build that the keychain hasn't been told to trust.
touch "$ROOT/app/build/.metadata_never_index"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$APP" >/dev/null 2>&1 || true
echo "app: $APP"

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
echo "app: $ROOT/app/build/DerivedData/Build/Products/$CONFIG/Meridian.app"

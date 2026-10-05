#!/bin/bash
# Builds the Rust core as a static library, generates Swift bindings with
# uniffi-bindgen-swift (library mode), wraps the library in an XCFramework,
# and installs both into app/Packages/MeridianCore.
#
#   scripts/build-core.sh            # debug core (fast iteration)
#   scripts/build-core.sh release    # optimized core (perf runs, shipping)
set -euo pipefail
PROFILE="${1:-debug}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORE="$ROOT/core"
PKG="$ROOT/app/Packages/MeridianCore"
TARGET="aarch64-apple-darwin"

cd "$CORE"
if [ "$PROFILE" = "release" ]; then
  cargo build -p meridian-ffi --release --target "$TARGET"
else
  cargo build -p meridian-ffi --target "$TARGET"
fi
cargo build -q -p uniffi-bindgen-swift
LIB="$CORE/target/$TARGET/$PROFILE/libmeridian_ffi.a"
BINDGEN="$CORE/target/debug/uniffi-bindgen-swift"

GEN="$(mktemp -d)"
trap 'rm -rf "$GEN"' EXIT
mkdir -p "$GEN/swift" "$GEN/headers"
"$BINDGEN" --swift-sources "$LIB" "$GEN/swift"
"$BINDGEN" --headers "$LIB" "$GEN/headers"
"$BINDGEN" --modulemap --module-name meridianFFI --modulemap-filename module.modulemap "$LIB" "$GEN/headers"

rm -rf "$PKG/MeridianCoreFFI.xcframework"
xcodebuild -create-xcframework -library "$LIB" -headers "$GEN/headers" -output "$PKG/MeridianCoreFFI.xcframework" >/dev/null

mkdir -p "$PKG/Sources/MeridianCore"
cp "$GEN/swift/"*.swift "$PKG/Sources/MeridianCore/"
echo "core ($PROFILE) → $PKG"
ls "$PKG/Sources/MeridianCore"

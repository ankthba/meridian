#!/bin/bash
# Renders function screens to PNG with deterministic mock data.
#   scripts/capture.sh "DES|AAPL US Equity;GP|AAPL US Equity|range=1Y" [out_dir] [WxH]
# Output defaults to reference/compare/ours/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPECS="${1:?specs like 'DES|AAPL US Equity'}"
OUT="${2:-$ROOT/reference/compare/ours}"
SIZE="${3:-960x600}"
APP="$ROOT/app/build/DerivedData/Build/Products/Debug/Meridian.app/Contents/MacOS/Meridian"
[ -x "$APP" ] || { echo "build the app first (xcodebuild ... build)"; exit 1; }
MERIDIAN_MODE=mock MERIDIAN_IN_MEMORY=1 MERIDIAN_FIXED_CLOCK_NS="${MERIDIAN_FIXED_CLOCK_NS:-1791216000000000000}" \
MERIDIAN_RESET_LAYOUT=1 MERIDIAN_SNAPSHOT_DIR="$OUT" MERIDIAN_SNAPSHOT_SPECS="$SPECS" MERIDIAN_SNAPSHOT_SIZE="$SIZE" \
MERIDIAN_SNAPSHOT_MAIN="${MERIDIAN_SNAPSHOT_MAIN:-0}" "$APP"
ls "$OUT"

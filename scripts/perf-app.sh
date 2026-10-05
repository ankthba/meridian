#!/bin/bash
# Measures app-level budgets (cold launch, command latency, 2,000-symbol
# streaming fps/CPU, chart pan/zoom) with a release build.
#   scripts/perf-app.sh            # writes bench/results/app-<date>.json
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/build-app.sh" release
APP="$ROOT/app/build/DerivedData/Build/Products/Release/Meridian.app/Contents/MacOS/Meridian"
OUT="$ROOT/bench/results/app-$(date +%F).json"
mkdir -p "$ROOT/bench/results"
MERIDIAN_PERF=1 MERIDIAN_MODE=mock MERIDIAN_IN_MEMORY=1 MERIDIAN_RESET_LAYOUT=1 MERIDIAN_MOCK_EXTRA=2000 MERIDIAN_MOCK_RATE=3 \
  MERIDIAN_PERF_OUT="$OUT" "$APP"
echo "→ $OUT"

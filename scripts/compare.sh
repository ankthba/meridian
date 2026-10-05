#!/bin/bash
# For every reference/<FUNCTION>/*.png with a matching render in
# reference/compare/ours/, writes reference/compare/<FUNCTION>-<name>.png.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
shopt -s nullglob
found=0
for ref in "$ROOT"/reference/*/*.png; do
  fn="$(basename "$(dirname "$ref")")"
  [ "$fn" = "compare" ] && continue
  ours="$(ls "$ROOT/reference/compare/ours/${fn}"*.png 2>/dev/null | head -1 || true)"
  [ -z "$ours" ] && { echo "no render for $fn"; continue; }
  out="$ROOT/reference/compare/${fn}-$(basename "$ref")"
  swift "$ROOT/scripts/compare.swift" "$ref" "$ours" "$out"
  found=1
done
[ $found -eq 1 ] || echo "No reference screenshots yet (see reference/README.md)."

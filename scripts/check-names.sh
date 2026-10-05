#!/bin/bash
# Fails if the incumbent terminal vendor's name appears in app code, core
# code, or bundled assets. Docs are exempt (they cite UX conventions).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NAME="$(printf 'Ymxvb21iZXJn' | base64 -d)"
hits=$(grep -rIil "$NAME" "$ROOT/app/Meridian" "$ROOT/app/MeridianTests" "$ROOT/app/Packages/MeridianCore/Package.swift" "$ROOT/core/crates" "$ROOT/core/xtask" "$ROOT/core/uniffi-bindgen-swift" 2>/dev/null | grep -v "/target/" || true)
if [ -n "$hits" ]; then
  echo "Forbidden brand name found in:"; echo "$hits"; exit 1
fi
echo "check-names: ok"

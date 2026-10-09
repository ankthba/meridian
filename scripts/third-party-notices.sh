#!/bin/bash
# Regenerates app/Meridian/Resources/ThirdPartyNotices.txt: copyright and
# license notices for every Rust crate and bundled C/C++ library compiled into
# the app's core library. Fails if any license isn't permissive.
#
#   scripts/third-party-notices.sh           # write the file, print a summary
#   scripts/third-party-notices.sh --check   # fail if the committed file is stale (CI)
#
# Needs cargo and python3 (standard library only).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 -I "$ROOT/scripts/third-party-notices.py" --root "$ROOT" "$@"

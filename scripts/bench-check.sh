#!/bin/bash
# Runs the Rust criterion benches and compares mean times against
# bench/baseline.json. Fails if any benchmark regresses by more than
# THRESHOLD percent (default 10). `--update` rewrites the baseline.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
THRESHOLD="${THRESHOLD:-10}"
cd "$ROOT/core"
cargo bench -q -p meridian-stream -p meridian-command -p meridian-provider-mock >/dev/null 2>&1 || cargo bench -q -p meridian-stream -p meridian-command >/dev/null
python3 - "$ROOT" "$THRESHOLD" "${1:-}" <<'PY'
import json, os, sys, glob, datetime
root, threshold, mode = sys.argv[1], float(sys.argv[2]), sys.argv[3]
results = {}
for f in glob.glob(os.path.join(root, "core/target/criterion/**/new/estimates.json"), recursive=True):
    name = os.path.relpath(os.path.dirname(os.path.dirname(f)), os.path.join(root, "core/target/criterion"))
    results[name] = json.load(open(f))["mean"]["point_estimate"]  # nanoseconds
os.makedirs(os.path.join(root, "bench/results"), exist_ok=True)
stamp = datetime.date.today().isoformat()
json.dump(results, open(os.path.join(root, f"bench/results/{stamp}.json"), "w"), indent=1, sort_keys=True)
base_path = os.path.join(root, "bench/baseline.json")
if mode == "--update" or not os.path.exists(base_path):
    json.dump(results, open(base_path, "w"), indent=1, sort_keys=True)
    print(f"baseline written ({len(results)} benches)")
    sys.exit(0)
base = json.load(open(base_path))
bad = []
for k, v in sorted(results.items()):
    b = base.get(k)
    if b is None:
        print(f"  new   {k}: {v/1000:.1f} µs"); continue
    pct = (v - b) / b * 100
    flag = "REGRESSION" if pct > threshold else "ok"
    print(f"  {flag:10} {k}: {v/1000:.2f} µs ({pct:+.1f}%)")
    if pct > threshold: bad.append(k)
if bad:
    print(f"FAILED: {len(bad)} regression(s) over {threshold}%"); sys.exit(1)
print("bench-check: ok")
PY

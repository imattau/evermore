#!/usr/bin/env bash
# Fail when the operation catalogue changed without a matching adapter release.
#
# On success the lock's catalogue fingerprint equals the current catalogue.
# Pass --require-adapter to also fail while the adapter fields are empty (use
# this once the ISO is expected to ship the default pair).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LOCK="$SCRIPT_DIR/registry.lock"
REQUIRE_ADAPTER=0
[[ "${1:-}" == "--require-adapter" ]] && REQUIRE_ADAPTER=1

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
[[ -f "$LOCK" ]] || { echo "missing $LOCK" >&2; exit 1; }

current="$(cd "$REPO_ROOT" && cargo run --offline -q -p evermore-adapter-training -- --fingerprint)"
locked="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["catalogue_fingerprint"])' "$LOCK")"

if [[ "$current" != "$locked" ]]; then
  cat >&2 <<EOF
operation catalogue changed: $locked -> $current

The adapter is no longer bound to the current registry. Regenerate the
dataset, retrain, re-evaluate, publish the signed model package, and update
$LOCK in the same change.
EOF
  exit 1
fi

if [[ "$REQUIRE_ADAPTER" == 1 ]]; then
  python3 - "$LOCK" <<'PY'
import json, sys
lock = json.load(open(sys.argv[1]))
missing = [key for key in ("adapter_package_id", "adapter_version", "eval_report_sha256") if not lock.get(key)]
if missing:
    raise SystemExit("adapter release is not recorded in registry.lock: " + ", ".join(missing))
PY
fi

echo "registry lock OK: $current"

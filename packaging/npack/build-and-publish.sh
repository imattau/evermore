#!/usr/bin/env bash
# Builds one Evermore component's release binary, assembles it with its
# systemd unit (if it has one) into the payload layout `evermore-npack
# publish` expects, and publishes it.
#
# Usage:
#   packaging/npack/build-and-publish.sh sh.evermore.settingsd \
#     --identity publisher --blossom-url https://blossom.example.org \
#     [--relay ws://127.0.0.1:4869]
#
# Prerequisites (see PROJECT_IDEA.md Phase 5):
#   - evermore-signer serving the --identity used here, with that
#     identity's key already provisioned.
#   - evermore-relay-supervisor running locally (or --relay pointed at
#     wherever it is).
#   - A Blossom server reachable at --blossom-url.
#   - Anyone who will `install`/`update` this package must first run
#     `evermore-npack trust <publisher-pubkey>` -- there is no default
#     trust seed yet (M5.3 is deliberately deferred until there's a real
#     publisher identity to seed with).
set -euo pipefail

PACKAGE_ID="${1:?usage: $0 PACKAGE_ID --identity NAME --blossom-url URL [--relay URL]}"
shift

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PKG_DIR="$ROOT/packaging/npack/$PACKAGE_ID"
MANIFEST="$PKG_DIR/manifest.json"
if [ ! -f "$MANIFEST" ]; then
  echo "no manifest for '$PACKAGE_ID' at $MANIFEST" >&2
  exit 1
fi

# "sh.evermore.blossom-fs" -> "evermore-blossom-fs", matching both the
# crate/app directory name and its binary target name.
CRATE_NAME="${PACKAGE_ID#sh.evermore.}"
BIN_NAME="evermore-$CRATE_NAME"

echo "building $BIN_NAME --release..."
cargo build --release --bin "$BIN_NAME" --manifest-path "$ROOT/Cargo.toml"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cp "$ROOT/target/release/$BIN_NAME" "$WORK/$BIN_NAME"

UNIT="$(sed -n 's/.*"systemd_unit": *"\([^"]*\)".*/\1/p' "$MANIFEST")"
if [ -n "$UNIT" ]; then
  cp "$ROOT/packaging/systemd/$UNIT" "$WORK/$UNIT"
fi

echo "publishing $PACKAGE_ID..."
cargo run --release -p evermore-npack -- publish "$MANIFEST" "$WORK" "$@"

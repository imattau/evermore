#!/usr/bin/env bash
# Packs one already-built Evermore component as a real npack (.npk) package
# for baking into the VM/ISO image's /etc/skel -- so npackd's own package
# database (and therefore npack-gui) has an honest record of it, instead of
# showing nothing installed while the binary sits on disk only because
# install-image.sh copied it there directly.
#
# This is the same `npack init` + pack machinery as
# packaging/npack/build-and-publish.sh, trimmed to what an offline,
# local-only image build needs: no cargo build (the caller already built
# release binaries), no GitHub upload, no Nostr publish, and -- unlike
# build-and-publish.sh -- no `register-service` post-install action. These
# packages are being installed at build time into a *skeleton* home, not by
# a user running `npack install` against a live system, and register-service
# writes a unit file into ~/.config/systemd/user/, which takes priority over
# /usr/lib/systemd/user/ in systemd's own search order -- exactly where
# evermore-enroll's enable_identity_services() already places the correct,
# live symlink for every real identity. Shipping a second, static copy here
# would shadow that symlink's target the moment it drifted from what's
# actually installed, silently reintroducing whatever the live unit last
# fixed. The binary/manifest record is still real and still shows up in
# npack-gui; it just doesn't touch service wiring, which install-image.sh
# and evermore-enroll already own.
#
# Usage: pack-for-image.sh <package> <version> <root> <stage-bin-dir> <output.npk>
set -euo pipefail

PACKAGE="${1:?usage: $0 PACKAGE VERSION ROOT STAGE_BIN_DIR OUTPUT_NPK}"
VERSION="${2:?usage: $0 PACKAGE VERSION ROOT STAGE_BIN_DIR OUTPUT_NPK}"
ROOT="${3:?usage: $0 PACKAGE VERSION ROOT STAGE_BIN_DIR OUTPUT_NPK}"
STAGE_BIN="${4:?usage: $0 PACKAGE VERSION ROOT STAGE_BIN_DIR OUTPUT_NPK}"
OUTPUT="${5:?usage: $0 PACKAGE VERSION ROOT STAGE_BIN_DIR OUTPUT_NPK}"

NPACK="${NPACK_BIN:-npack}"
PUBLISHER="$(python3 -c 'import json; print(json.load(open("'"$ROOT"'/packaging/npack/publisher.json"))["npub"])')"
BIN_NAME="evermore-$PACKAGE"

case "$PACKAGE" in
  settingsd|notifyd|blossom-fs|contextd|indexd|inferd|operationsd|agentd|sway-ai-bridge|mcp|panel) ;;
  *) echo "unknown package '$PACKAGE' -- see build-and-publish.sh's own list" >&2; exit 1 ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SOURCE="$WORK/source"

"$NPACK" init "$SOURCE" --name "$PACKAGE" --version "$VERSION" --publisher "$PUBLISHER" >/dev/null

mkdir -p "$SOURCE/bin"
install -m 0755 "$STAGE_BIN/$BIN_NAME" "$SOURCE/bin/$BIN_NAME"

mkdir -p "$(dirname "$OUTPUT")"
"$NPACK" pack "$SOURCE" --output "$OUTPUT" >/dev/null
echo "==> packed $OUTPUT"

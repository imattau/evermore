#!/usr/bin/env bash
# Builds one Evermore component's release binary, packs it as a real
# npack (.npk) package (https://github.com/imattau/npack -- not a
# component of this repo), uploads the archive as a GitHub Release
# asset in *this* repo, and publishes npack's release events pointing
# at that asset via --artifact-url -- no Blossom server needed.
#
# --artifact-url is a feature this repo added to npack
# (https://github.com/imattau/npack/pull/33, merged, released in
# v0.8.0): requires npack >= 0.8.0 on PATH, or NPACK_BIN pointed at one.
#
# Usage:
#   packaging/npack/build-and-publish.sh <package> \
#     --publisher <npub-or-hex> --tag v0.1.0 \
#     [--relay wss://relay.example ...] [--repo imattau/evermore] \
#     [--secret-key <nsec-or-hex>] [--dry-run]
#
# <package> is one of: settingsd, notifyd, blossom-fs, contextd, indexd,
# inferd, operationsd, agentd, sway-ai-bridge, mcp, panel. (Not greeter -- see
# README.md's caveat; not signer/relay/enroll/pkcs11 -- bootstrap-only,
# never npack packages at all.)
#
# Prerequisites: `npack` on PATH, `gh` authenticated against --repo, and
# either a registered publisher key (`npack register`, for interactive
# use) matching --publisher, or --secret-key passed explicitly (for CI,
# where nothing persists a registration between runs). --dry-run builds
# and packs but skips the GitHub upload and npack publish, so you can
# inspect the .npk and its manifest first.
set -euo pipefail

PACKAGE="${1:?usage: $0 PACKAGE --publisher KEY --tag vX.Y.Z [options]}"
shift

PUBLISHER=""
TAG=""
REPO="imattau/evermore"
RELAYS=()
SECRET_KEY=""
DRY_RUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --publisher) PUBLISHER="$2"; shift 2 ;;
    --tag) TAG="$2"; shift 2 ;;
    --repo) REPO="$2"; shift 2 ;;
    --relay) RELAYS+=("$2"); shift 2 ;;
    --secret-key) SECRET_KEY="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$PUBLISHER" ] || { echo "missing --publisher" >&2; exit 2; }
[ -n "$TAG" ] || { echo "missing --tag" >&2; exit 2; }
VERSION="${TAG#v}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
NPACK="${NPACK_BIN:-npack}"
BIN_NAME="evermore-$PACKAGE"

NPACK_VERSION="$("$NPACK" --version 2>/dev/null | awk '{print $2}')"
NPACK_MAJOR="${NPACK_VERSION%%.*}"
NPACK_MINOR="${NPACK_VERSION#*.}"
NPACK_MINOR="${NPACK_MINOR%%.*}"
if [ "${NPACK_MAJOR:-0}" -eq 0 ] && [ "${NPACK_MINOR:-0}" -lt 8 ]; then
  echo "npack $NPACK_VERSION is too old -- --artifact-url needs >= 0.8.0 (set NPACK_BIN to point at a newer build)" >&2
  exit 1
fi

# name, dependency names (space-separated, empty for none), systemd unit
# filename under packaging/systemd/ (empty for none -- mcp and panel
# aren't daemons).
case "$PACKAGE" in
  settingsd)      DEPS="";               UNIT="evermore-settingsd@.service" ;;
  notifyd)        DEPS="settingsd";      UNIT="evermore-notifyd@.service" ;;
  blossom-fs)     DEPS="";               UNIT="evermore-blossom-fs@.service" ;;
  contextd)       DEPS="";               UNIT="evermore-contextd.service" ;;
  indexd)         DEPS="";               UNIT="evermore-indexd.service" ;;
  inferd)         DEPS="";               UNIT="evermore-inferd.service" ;;
  operationsd)    DEPS="settingsd";      UNIT="evermore-operationsd@.service" ;;
  agentd)         DEPS="operationsd";    UNIT="evermore-agentd@.service" ;;
  sway-ai-bridge) DEPS="contextd";       UNIT="evermore-sway-ai-bridge.service" ;;
  mcp)            DEPS="operationsd";    UNIT="" ;;
  panel)          DEPS="";               UNIT="" ;;
  *) echo "unknown package '$PACKAGE' -- see this script's own usage comment" >&2; exit 1 ;;
esac

echo "building $BIN_NAME --release..."
cargo build --release --bin "$BIN_NAME" --manifest-path "$ROOT/Cargo.toml"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SOURCE="$WORK/source"

echo "scaffolding npack manifest ($PACKAGE $VERSION, publisher $PUBLISHER)..."
"$NPACK" init "$SOURCE" --name "$PACKAGE" --version "$VERSION" --publisher "$PUBLISHER"

mkdir -p "$SOURCE/bin"
install -m 0755 "$ROOT/target/release/$BIN_NAME" "$SOURCE/bin/$BIN_NAME"

if [ -n "$UNIT" ]; then
  mkdir -p "$SOURCE/lib/systemd/user"
  cp "$ROOT/packaging/systemd/$UNIT" "$SOURCE/lib/systemd/user/$UNIT"
fi

python3 - "$SOURCE/.npack/manifest.json" "$PUBLISHER" "$UNIT" $DEPS <<'PY'
import json, sys
manifest_path, publisher, unit, *deps = sys.argv[1:]
with open(manifest_path) as f:
    manifest = json.load(f)
manifest["dependencies"] = [
    {"publisher": publisher, "name": dep, "requirement": "*"} for dep in deps
]
if unit:
    manifest["post_install"] = [
        {"action": "register-service", "path": f"lib/systemd/user/{unit}"}
    ]
with open(manifest_path, "w") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
PY

ARTIFACT_NAME="evermore-$PACKAGE-$VERSION-linux-x86_64.npk"
echo "packing $ARTIFACT_NAME..."
"$NPACK" pack "$SOURCE" --output "$WORK/$ARTIFACT_NAME"
"$NPACK" manifest "$WORK/$ARTIFACT_NAME" --output "$WORK/manifest.json"
"$NPACK" verify "$WORK/manifest.json"

echo "built $WORK/$ARTIFACT_NAME"
echo "manifest: $WORK/manifest.json"
if [ "$DRY_RUN" = "1" ]; then
  echo "dry run -- stopping before upload/publish. Files kept at:"
  cp -r "$WORK" "$ROOT/$(basename "$WORK").dry-run"
  echo "  $ROOT/$(basename "$WORK").dry-run"
  trap - EXIT
  exit 0
fi

echo "uploading $ARTIFACT_NAME to $REPO release $TAG..."
gh release upload "$TAG" "$WORK/$ARTIFACT_NAME" --repo "$REPO" --clobber
ARTIFACT_URL="https://github.com/$REPO/releases/download/$TAG/$ARTIFACT_NAME"

RELAY_ARGS=()
for relay in "${RELAYS[@]}"; do
  RELAY_ARGS+=(--relay "$relay")
done

SECRET_ARGS=()
if [ -n "$SECRET_KEY" ]; then
  SECRET_ARGS=(--secret-key "$SECRET_KEY")
fi

echo "publishing $PACKAGE $VERSION -> $ARTIFACT_URL..."
"$NPACK" publish "$WORK/manifest.json" "${RELAY_ARGS[@]}" "${SECRET_ARGS[@]}" --artifact-url "$ARTIFACT_URL"

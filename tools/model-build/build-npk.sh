#!/usr/bin/env bash
# Build an .npk model package from a directory containing manifest.json and
# the declared GGUF artifacts.
#
#   build-npk.sh <package-dir> <output.npk>
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <package-dir> <output.npk>" >&2
  exit 2
fi

PACKAGE="$1"
OUTPUT="$2"

[[ -f "$PACKAGE/manifest.json" ]] || {
  echo "missing $PACKAGE/manifest.json" >&2
  exit 1
}

python3 - "$PACKAGE" <<'PY'
import hashlib
import json
import os
import sys

package = sys.argv[1]
with open(os.path.join(package, "manifest.json"), encoding="utf-8") as handle:
    manifest = json.load(handle)

for field in ("id", "name", "version"):
    if not manifest.get(field):
        raise SystemExit(f"manifest is missing {field}")

payload = manifest.get("ai_model")
if not payload:
    raise SystemExit("manifest has no ai_model payload")


def check(file_field, hash_field):
    name = payload.get(file_field)
    expected = payload.get(hash_field)
    if not name or not expected:
        raise SystemExit(f"ai_model must declare {file_field} and {hash_field}")
    path = os.path.join(package, name)
    if not os.path.isfile(path):
        raise SystemExit(f"missing artifact: {path}")
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    actual = digest.hexdigest()
    if actual.lower() != expected.lower():
        raise SystemExit(f"{name}: SHA-256 is {actual}, manifest says {expected}")
    print(f"verified {name}: {actual}")


check("gguf_file", "sha256")
if payload.get("adapter_gguf_file") or payload.get("adapter_sha256"):
    check("adapter_gguf_file", "adapter_sha256")
    if not payload.get("operation_registry_version"):
        raise SystemExit("an adapter package must declare operation_registry_version")
    training = payload.get("training") or manifest.get("training")
    if not training:
        raise SystemExit("an adapter package must declare training provenance")
    for key in (
        "base_model_sha256",
        "dataset_sha256",
        "config_sha256",
        "catalogue_fingerprint",
    ):
        value = training.get(key, "")
        if len(value) != 64 or any(character not in "0123456789abcdefABCDEF" for character in value):
            raise SystemExit(f"training.{key} must be a 64-character SHA-256")
    report = training.get("eval_report_sha256")
    if report and (
        len(report) != 64 or any(character not in "0123456789abcdefABCDEF" for character in report)
    ):
        raise SystemExit("training.eval_report_sha256 must be a 64-character SHA-256")

print(f"package {manifest['id']} {manifest['version']}")
PY

mkdir -p "$(dirname "$OUTPUT")"
tar -czf "$OUTPUT" -C "$PACKAGE" .

echo "==> wrote $OUTPUT"
sha256sum "$OUTPUT"

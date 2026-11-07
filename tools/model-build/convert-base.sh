#!/usr/bin/env bash
# Convert a Hugging Face base model to GGUF and apply a quant.
#
#   convert-base.sh <hf-model-dir> <output.gguf> [quant] 
#
# quant defaults to Q4_K_M; pass F16 to keep the converted file unquantized.
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: $0 <hf-model-dir> <output.gguf> [quant]" >&2
  exit 2
fi

SOURCE="$1"
OUTPUT="$2"
QUANT="${3:-Q4_K_M}"
LLAMA_CPP_SRC="${LLAMA_CPP_SRC:-$HOME/src/llama.cpp}"
PYTHON="${PYTHON:-$LLAMA_CPP_SRC/venv/bin/python}"
[[ -x "$PYTHON" ]] || PYTHON="python3"

CONVERT="$LLAMA_CPP_SRC/convert_hf_to_gguf.py"
QUANTIZE="${LLAMA_QUANTIZE:-llama-quantize}"

[[ -d "$SOURCE" ]] || { echo "missing model directory: $SOURCE" >&2; exit 1; }
[[ -f "$CONVERT" ]] || { echo "missing $CONVERT (set LLAMA_CPP_SRC)" >&2; exit 1; }
command -v "$QUANTIZE" >/dev/null || {
  echo "missing llama-quantize (install Arch's llama-cpp package)" >&2
  exit 1
}
mkdir -p "$(dirname "$OUTPUT")"

workdir="$(mktemp -d /tmp/evermore-model.XXXXXX)"
trap 'rm -rf "$workdir"' EXIT
intermediate="$workdir/base-f16.gguf"

echo "==> converting $SOURCE to F16 GGUF"
"$PYTHON" "$CONVERT" "$SOURCE" --outfile "$intermediate" --outtype f16

if [[ "$QUANT" == "F16" ]]; then
  mv "$intermediate" "$OUTPUT"
else
  echo "==> quantizing to $QUANT"
  "$QUANTIZE" "$intermediate" "$OUTPUT" "$QUANT"
fi

echo "==> wrote $OUTPUT"
sha256sum "$OUTPUT"

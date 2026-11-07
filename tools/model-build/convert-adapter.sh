#!/usr/bin/env bash
# Convert a PEFT LoRA adapter to a GGUF the base model can load.
#
#   convert-adapter.sh <peft-adapter-dir> <output.gguf> [extra convert args...]
#
# Adapters stay F16. Record the trained operation-registry version in the
# package manifest; the model store refuses an adapter without one.
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: $0 <peft-adapter-dir> <output.gguf> [extra args...]" >&2
  exit 2
fi

SOURCE="$1"
OUTPUT="$2"
shift 2
LLAMA_CPP_SRC="${LLAMA_CPP_SRC:-$HOME/src/llama.cpp}"
PYTHON="${PYTHON:-$LLAMA_CPP_SRC/venv/bin/python}"
[[ -x "$PYTHON" ]] || PYTHON="python3"
CONVERT="$LLAMA_CPP_SRC/convert_lora_to_gguf.py"

[[ -d "$SOURCE" ]] || { echo "missing adapter directory: $SOURCE" >&2; exit 1; }
[[ -f "$CONVERT" ]] || { echo "missing $CONVERT (set LLAMA_CPP_SRC)" >&2; exit 1; }
mkdir -p "$(dirname "$OUTPUT")"

echo "==> converting LoRA adapter $SOURCE to F16 GGUF"
"$PYTHON" "$CONVERT" "$SOURCE" --outfile "$OUTPUT" --outtype f16 "$@"

echo "==> wrote $OUTPUT"
sha256sum "$OUTPUT"

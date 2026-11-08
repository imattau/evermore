#!/usr/bin/env bash
# Install the optional Evermore training toolchain on an Arch-based machine.
# Not part of the default ISO; see packaging/training/README.md.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/evermore/training"
VENV="$DATA_HOME/venv"
LLAMA_SRC="$DATA_HOME/llama.cpp"
INSTALL_PACKAGES=0
AMD=0

for arg in "$@"; do
  case "$arg" in
    --install-packages) INSTALL_PACKAGES=1 ;;
    --amd) AMD=1 ;;
    --help | -h)
      echo "usage: $0 [--install-packages] [--amd]"
      exit 0
      ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
mkdir -p "$DATA_HOME"

if [[ "$INSTALL_PACKAGES" == 1 ]]; then
  command -v pacman >/dev/null || { echo "pacman not found; not an Arch system" >&2; exit 1; }
  if [[ "$AMD" == 1 ]]; then
    PACKAGES="$SCRIPT_DIR/arch-packages-amd.txt"
  else
    PACKAGES="$SCRIPT_DIR/arch-packages-nvidia.txt"
  fi
  # shellcheck disable=SC2046
  sudo pacman -S --needed --noconfirm $(grep -v '^#' "$PACKAGES")
fi

if [[ ! -x "$VENV/bin/python" ]]; then
  echo "==> creating venv at $VENV"
  python3 -m venv "$VENV"
fi
"$VENV/bin/pip" install -q --upgrade pip

if [[ "$AMD" == 1 ]]; then
  : "${TORCH_INDEX_URL:?set TORCH_INDEX_URL to a ROCm wheel index before running with --amd}"
  "$VENV/bin/pip" install -q torch --index-url "$TORCH_INDEX_URL"
fi
echo "==> installing training requirements"
"$VENV/bin/pip" install -q -r "$REPO_ROOT/tools/adapter-training/requirements.txt"

if [[ ! -d "$LLAMA_SRC" ]]; then
  echo "==> cloning llama.cpp (conversion scripts only)"
  git clone --depth 1 https://github.com/ggml-org/llama.cpp "$LLAMA_SRC"
fi

if "$VENV/bin/python" -c 'import torch; raise SystemExit(0 if torch.cuda.is_available() else 1)' 2>/dev/null; then
  echo "==> CUDA/ROCm is visible to PyTorch"
else
  echo "warning: PyTorch cannot see a GPU; training will be CPU-only" >&2
fi

cat <<EOF

Training profile ready.

  export LLAMA_CPP_SRC="$LLAMA_SRC"
  export PYTHON="$VENV/bin/python"

Then follow "Run the pipeline" in packaging/training/README.md. Training data,
models, and adapters stay local unless explicitly exported.
EOF

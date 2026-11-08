# Evermore training profile

Optional toolchain for producing the operation LoRA on an Arch/Evermore
machine with a GPU. This profile is **not part of the default ISO**: training
is a publisher/build step, and the signed adapter is what ships to users.

## Requirements

- Arch-based system with `pacman`.
- NVIDIA: recent driver with CUDA support (Blackwell needs the open modules).
- AMD: ROCm supporting the card.
- ~15 GB for the venv, `llama.cpp`, and model/adapter artifacts.
- At least 8 GB VRAM for a 1B–3B LoRA; 16 GB is comfortable.

## Install

```sh
sudo packaging/training/setup.sh --install-packages
```

or, if the system packages are already present:

```sh
packaging/training/setup.sh
```

The script:

1. checks `pacman` and Python;
2. installs `arch-packages-nvidia.txt` (or `arch-packages-amd.txt` with
   `--amd`);
3. creates a venv under `$XDG_DATA_HOME/evermore/training/venv`;
4. installs `tools/adapter-training/requirements.txt`;
5. clones `llama.cpp` under `$XDG_DATA_HOME/evermore/training/llama.cpp` for
   the conversion scripts;
6. prints the pipeline commands with the right `PYTHON`/`LLAMA_CPP_SRC`.

No model, dataset, or adapter is uploaded by the setup script.

## Run the pipeline

```sh
export LLAMA_CPP_SRC="$HOME/.local/share/evermore/training/llama.cpp"
export PYTHON="$HOME/.local/share/evermore/training/venv/bin/python"

cargo run -p evermore-adapter-training -- --output-dir tools/adapter-training/data
"$PYTHON" tools/adapter-training/train_lora.py \
  --config tools/adapter-training/config.example.json \
  --data-dir tools/adapter-training/data \
  --output-dir tools/adapter-training/out/adapter-peft
tools/model-build/convert-adapter.sh \
  tools/adapter-training/out/adapter-peft \
  tools/adapter-training/out/adapter.gguf
tools/adapter-training/evaluate.py \
  --eval tools/adapter-training/data/eval.jsonl \
  --base out/base.gguf \
  --adapter tools/adapter-training/out/adapter.gguf \
  --scale 0,0.5,1.0 \
  --grammar tools/adapter-training/proposal.gbnf \
  --out tools/adapter-training/out/eval-report.json
```

Training data stays local unless the operator explicitly exports it. See
`tools/adapter-training/README.md` for provenance fields and the registry gate.

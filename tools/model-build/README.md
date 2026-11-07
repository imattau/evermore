# Model build recipes

Build-time conversion and `npack` packaging for signed model artifacts. None of
these scripts run at runtime; `evermore-inferd` only reads the resulting store
manifest and GGUFs.

## Prerequisites

- Arch's `llama-cpp` package (provides `llama-quantize` and `llama-server`).
- A `llama.cpp` source checkout for the conversion scripts, which Arch does not
  package:
  `git clone https://github.com/ggml-org/llama.cpp ~/src/llama.cpp`
- Python with the checkout's requirements installed (use a venv inside the
  checkout, or set `PYTHON`).
- `tar` and `sha256sum`.

Set `LLAMA_CPP_SRC` if the checkout is not at `~/src/llama.cpp`.

## Quantization policy

| Artifact | Default | Alternatives | Notes |
| :--- | :--- | :--- | :--- |
| Base model | `Q4_K_M` | `Q5_K_M`, `Q8_0` | ISO default for 1B–3B models; `Q8_0` for users who prefer quality over size. |
| LoRA adapter | `F16` | none | Do not quantize an adapter; keep it `F16` so the base quant stays independent. |
| Context | 8192 | 16384 | KV cache is the small-device memory constraint; raise only after measuring. |

## Convert a base model

```sh
tools/model-build/convert-base.sh \
  ~/models/TinyLlama-1.1B-Chat-v1.0 \
  out/base.gguf \
  Q4_K_M
```

The script converts with `convert_hf_to_gguf.py` at `F16`, then quantizes with
`llama-quantize`. Pass `F16` to skip quantization.

## Convert a LoRA adapter

```sh
tools/model-build/convert-adapter.sh \
  ~/training/evermore-operations-lora \
  out/adapter.gguf
```

The adapter must be trained against a pinned operation-registry version. Record
that version in the package manifest's `operation_registry_version`; an adapter
without it is rejected by the model store.

## Package an npack model

The package directory needs a `manifest.json` at its root (see
`model-manifest.example.json`) plus the GGUFs:

```text
evermore-model/
  manifest.json
  base.gguf
  adapter.gguf
```

Then:

```sh
tools/model-build/build-npk.sh evermore-model dist/evermore-model-1.0.0.npk
```

The script verifies the declared SHA-256 values against the files and prints
the archive hash. Sign the kind:30063 release event with that hash in its `x`
tag using the publisher's normal `npack` flow. There is no central model
registry: installation discovers the release through the publisher trust list.

## Default ISO pair

The intended ISO pre-install is a `Q4_K_M` base in the 1B–3B range plus the
`F16` Evermore adapter, both shipped as signed `npack` packages and imported
through `ai.model.install_signed_npack`.

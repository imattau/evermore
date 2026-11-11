# evermore-inferd

Local inference supervisor with a stable, versioned newline-JSON Unix socket
API. `evermore-inferd` does not link `llama.cpp`: it owns model storage,
manifest validation, and lifecycle, and spawns one `llama-server` child per
loaded model on an ephemeral loopback port. The daemon does not read arbitrary
files, relay events, sign Nostr events, or invoke operations.

## Run

Install Arch's `llama-cpp` package (which provides `llama-server`), place signed
model packages in the store, then start:

```sh
evermore-inferd
```

System models are read from `/var/lib/evermore-models`; user models from
`$XDG_DATA_HOME/evermore/models` (normally
`~/.local/share/evermore/models`). The user store shadows a system model with
the same name. `EVERMORE_MODEL_DIRS` overrides the search path with a
colon-separated list. `EVERMORE_MODEL_IMPORT_DIR` sets where
`InstallNpackModel` publishes imported artifacts.

Other settings: `EVERMORE_LLAMA_SERVER_BIN` (default `llama-server`),
`EVERMORE_INFER_MAX_MODELS` (default 2, clamped to 1–8),
`EVERMORE_INFER_READY_TIMEOUT_SECS` (default 120), and
`EVERMORE_INFER_LOG_DIR` for child logs (default
`$XDG_RUNTIME_DIR/evermore/llama-server`). The user service unit is
`evermore-inferd.service`.

## Socket protocol

Clients use `evermore_common::ai::{Request, Event}` at protocol version 2.
Requests support health, listing installed models, unloading a model, adapter
inspection and scale changes, signed package import, embeddings, and streaming
chat. Chat events include `started`, `token`, and `completed`; a request ID
correlates every event.

`Backend::Cpu` launches the child with `--n-gpu-layers 0`, `Gpu` with
`--n-gpu-layers all`, and `Auto` leaves placement to `llama.cpp`. A request for
a different backend restarts that model's child. `UnloadModel` stops the child;
it does not delete anything from the store.

## Adapters and grammars

When a manifest declares an adapter, the child is launched with `--lora` (or
`--lora-scaled` for a non-default scale). `ListAdapters` reports the declared
adapter with `loaded: false` until the model runs; `SetAdapterScale` starts the
child if needed and posts to `llama-server`'s `/lora-adapters` API. A scale is
runtime state only: restarting or unloading the model returns it to the
manifest's `scale`.

`Chat` accepts an optional GBNF `grammar` (64 KiB maximum), used for
operation-registry JSON proposals. This build of `llama-server` (Arch
`llama-cpp` 0.6.0) rejects `json_schema` and `response_format` with a sampler
initialization error, so callers should convert a JSON schema to GBNF before
calling. A grammar constrains model output; it grants no authority.

There is no pull, push, remote provider, or cloud-consent API. `:cloud` model
names are rejected.

## Model store

Each installed model version is one directory:

```text
<root>/<package>-<version>/
  manifest.json
  base.gguf
  adapter.gguf    optional LoRA
```

`manifest.json` is `evermore_common::ai::ModelManifest`:

```json
{
  "manifest_version": 1,
  "package_id": "p7.model.tinyllama",
  "package_version": "1.0.0",
  "description": "TinyLlama chat",
  "base": { "file": "base.gguf", "sha256": "<64 hex>" },
  "adapter": {
    "file": "adapter.gguf",
    "sha256": "<64 hex>",
    "scale": 1.0
  },
  "context_size": 2048,
  "quant": "Q4_K_M",
  "capabilities": ["chat"],
  "operation_registry_version": "2026-11-08",
  "training": {
    "base_model_sha256": "<64 hex>",
    "dataset_sha256": "<64 hex>",
    "config_sha256": "<64 hex>",
    "eval_report_sha256": "<64 hex>",
    "catalogue_fingerprint": "<64 hex>",
    "contributed_traces": false
  }
}
```

An adapter requires `operation_registry_version` — the registry version it was
trained against — so the daemon refuses to pair it with an unrelated base. The
optional `training` block records the base, dataset, config, eval report, and
catalogue fingerprint; `tools/adapter-training/` produces it. The scanner
validates structure and path containment; content hashes are verified when a
signed package is imported, not re-hashed on every start.

## Signed npack import

`InstallNpackModel { package_id }` reads an already installed package, verifies
its retained signed kind:30063 release event, package/version binding, archive
digest, and declared GGUF digests (base and adapter), then hard-links or copies
the artifacts into the model store with a manifest. It never downloads
packages. Call it only through the approval-required
`ai.model.install_signed_npack` operation in `evermore-operationsd`; the
inference socket itself is private to the user.

Npack manifests declare the payload:

```json
"ai_model": {
  "gguf_file": "models/example.gguf",
  "sha256": "<64-character SHA-256 of that GGUF file>",
  "adapter_gguf_file": "models/adapter.gguf",
  "adapter_sha256": "<64-character SHA-256>",
  "adapter_scale": 1.0,
  "context_size": 2048,
  "quant": "Q4_K_M",
  "capabilities": ["chat"],
  "operation_registry_version": "2026-11-08"
}
```

The model name is `evermore/<package-id>:<version>`.

Prompts are forwarded to the local child as supplied. Callers must apply
context scopes and shared secret redaction before constructing messages.

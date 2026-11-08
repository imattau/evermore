# Adapter training and evaluation

Build tooling for the Evermore operation LoRA: the adapter that makes tool
proposals follow the operation catalogue and its argument shapes. Nothing here
runs at runtime; `evermore-inferd` only reads the signed GGUF package.

Training hardware is opt-in and outside the ISO/build VM. The build VM only
validates and packages the resulting adapter.

## 1. Generate the dataset

```sh
cargo run -p evermore-adapter-training -- --output-dir tools/adapter-training/data
```

The generator reads `evermore_common::operations` directly, so every catalogue
operation is represented and the dataset cannot drift silently. It writes
`train.jsonl`, `eval.jsonl`, and `dataset-manifest.json` with the catalogue
fingerprint and counts. Splits are deterministic.

Generated records cover each operation's description, access/approval class,
canonical arguments, invalid-argument repair, and refusal cases. The assistant
target format is one JSON object:

```json
{"operation":"settings.get","arguments":{"domain":"theme","key":"mode"}}
```

or

```json
{"refusal":"no approved operation executes shell commands"}
```

### Opt-in local traces

Synthetic catalogue data needs no owner data. To include real operation
lifecycle examples, export a JSONL file locally, review it, then pass it:

```sh
cargo run -p evermore-adapter-training -- \
  --output-dir tools/adapter-training/data \
  --traces reviewed-traces.jsonl
```

Trace lines use:

```json
{"operation":"settings.get","arguments":{"domain":"theme","key":"mode"},"request":"read the theme"}
```

The generator applies `evermore_common::ai::redact_text` recursively to string
values and request text, validates each operation and its arguments against the
current catalogue, and records the trace file hash in the dataset manifest.
Nothing is uploaded automatically.

## 2. Train the adapter

On a CUDA/ROCm host:

```sh
python3 -m venv .venv && . .venv/bin/activate
pip install -r tools/adapter-training/requirements.txt

tools/adapter-training/train_lora.py \
  --config tools/adapter-training/config.example.json \
  --data-dir tools/adapter-training/data \
  --output-dir out/adapter-peft
```

The config pins base model, LoRA rank/alpha/dropout, learning rate, epochs, and
seed. Keep the file in the repository so the run is reproducible.

## 3. Evaluate

```sh
tools/adapter-training/evaluate.py \
  --eval tools/adapter-training/data/eval.jsonl \
  --base out/base.gguf \
  --out out/base-eval.json

tools/adapter-training/evaluate.py \
  --eval tools/adapter-training/data/eval.jsonl \
  --base out/base.gguf \
  --adapter out/adapter.gguf \
  --scale 0,0.5,1.0 \
  --grammar tools/adapter-training/proposal.gbnf \
  --out out/adapter-eval.json
```

Metrics: parse rate, tool-selection accuracy, argument/schema validity,
refusal accuracy, and proposals for non-existent operations. Compare
base-vs-adapter and scale runs before publishing. The evaluator starts its own
`llama-server`; it does not touch the user's model store.

## 4. Convert, package, and record provenance

```sh
tools/model-build/convert-adapter.sh out/adapter-peft out/adapter.gguf
```

Put the hash and dataset metadata in the package manifest's `ai_model.training`
block, then build the `.npk` with `tools/model-build/build-npk.sh`. The block
requires:

```json
"training": {
  "base_model_sha256": "<sha256>",
  "dataset_sha256": "<from dataset-manifest.json>",
  "config_sha256": "<sha256 of config.example.json>",
  "eval_report_sha256": "<sha256 of the eval report>",
  "catalogue_fingerprint": "<from dataset-manifest.json>",
  "contributed_traces": false
}
```

## 5. Registry gate

`registry.lock` pins the catalogue fingerprint and the adapter release expected
to match it:

```sh
tools/adapter-training/check-registry.sh
tools/adapter-training/check-registry.sh --require-adapter
```

If the catalogue changes, the check fails with a rebuild-and-re-evaluate
message until the lock and the signed adapter package are updated together.
Use `--require-adapter` once the ISO is expected to ship the default pair.

## Reference run (2026-11-08)

Local validation artifact, not a published or signed release:

- Base: `TinyLlama-1.1B-Chat-v1.0`, `Q4_K_M` GGUF.
- Data: 124 train / 72 eval records, catalogue fingerprint
  `68a86dfe…de1f`.
- Config: rank 32, alpha 64, 12 epochs, lr 3e-4, batch 1 × accumulate 4.
- Training: 372 steps, ~106 s on an RTX 5060 Ti; train loss 0.186, eval loss
  0.319, eval token accuracy 0.954.
- Evaluation (72 records, `proposal.gbnf`):

  | Run | Parse | Operation | Schema | Refusal |
  | :--- | ---: | ---: | ---: | ---: |
  | base, scale 0 | 0.764 | 0.423 | 0.099 | 0.0 |
  | adapter, scale 1.0 | 0.972 | 0.594 | 0.609 | 0.0 |

- Hot-swap verified through `evermore-inferd`: declared adapter reported
  unloaded, `SetAdapterScale` loaded it at 1.0, chat completed under a GBNF
  grammar, rescale to 0.25 was reported by `llama-server`, and unload stopped
  the child.

Refusal accuracy is the weak point: the base and adapter both propose an
operation for out-of-catalogue requests. Before publishing, add more refusal
variants (and likely a larger base model) and require a non-zero refusal score
in the evaluation gate.

# Adapter training and evaluation

Build tooling for Evermore's Qwen-specific tool-use LoRA. It teaches both the
panel's operation-proposal format and Evermore MCP's `tools/call` format,
including `mcp_status`, `op_status`, exact tool names, argument schemas,
approval-gated writes, and refusals for unsupported or unsafe requests.
Nothing here runs at runtime; `evermore-inferd` only reads the signed GGUF
package.

Before changing the dataset, model split, or release criteria, follow
[`REQUIREMENTS.md`](REQUIREMENTS.md). It separates MCP execution from the
panel's write-proposal path and deterministic agent recipes.

Training hardware is opt-in and outside the ISO/build VM. The build VM only
validates and packages the resulting adapter.

## 1. Generate the dataset

```sh
cargo run -p evermore-adapter-training -- --output-dir tools/adapter-training/data
```

The generator reads `evermore_common::operations` directly, so every catalogue
operation is represented and the dataset cannot drift silently. It writes
`train.jsonl`, `dev.jsonl`, `test.jsonl`, and `dataset-manifest.json` with the
catalogue fingerprint, content hash, and per-split task counts. Splits are
deterministic. Training reads only train and dev; keep test for the final
locked evaluation.

Panel examples are generated only for approval-gated writes, matching
`evermore-agentd propose`; read operations belong to deterministic recipes.
Generated records pair each request with naturally worded argument values,
vary those values across examples, and include invalid-argument repair and
refusal cases. The generator makes ten positive examples per tool in each
split, plus targeted training-only phrasings for MCP capabilities that need
clearer natural-language coverage and privacy-preserving calendar updates.
Its initial test split is template-derived and marked unreviewed. Replace that
split with the independently authored, catalogue-validated locked scenarios
before using it for release evaluation:

```sh
python3 tools/adapter-training/build-reviewed-test.py
```

The builder replaces `test.jsonl`, writes the human-readable scenario source,
and refreshes the manifest hashes; train and dev remain byte-identical. The
reviewed case source is
[`scenarios/locked-test-v1.jsonl`](scenarios/locked-test-v1.jsonl), with its
authoring and validation logic in `build-reviewed-test.py`. Run the builder
after every dataset regeneration, then use the locked test only for the final
release decision. MCP examples
include schema-rich, semantically related candidate tools so training tests
selection among Evermore tools, not just output formatting. The panel target
format includes the exact marker consumed by Evermore's proposal parser:

```json
EVERMORE_PROPOSAL: {"operation":"settings.set","arguments":{"domain":"theme","key":"mode","value":"dark"}}
```

or

```json
{"refusal":"no approved operation executes shell commands"}
```

MCP records instead use the shape expected by MCP `tools/call`:

```json
{"name":"settings.set","arguments":{"domain":"theme","key":"mode","value":"dark"}}
```

The MCP tool list and each tool's `inputSchema` are embedded in those examples.
Write tools submit owner-approval-gated requests; the assistant must never
claim that a proposed write has already executed.

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
  --config tools/adapter-training/config.qwen2.5-1.5b.json \
  --data-dir tools/adapter-training/data \
  --output-dir out/adapter-peft
```

`--task mcp` and `--task panel` train the two paths independently when
shared-task training interferes with tool selection or proposal validity;
the default `--task all` trains both together.

The config pins base model, LoRA rank/alpha/dropout, learning rate, epochs, and
seed. Keep the file in the repository so the run is reproducible. Training
uses prompt/completion records with `completion_only_loss=True`: only assistant
responses contribute to the loss. This avoids spending most of the objective
on repeated MCP catalogues and system instructions embedded in each prompt.

## 3. Evaluate

```sh
tools/adapter-training/evaluate.py \
  --dev tools/adapter-training/data/dev.jsonl \
  --dataset-manifest tools/adapter-training/data/dataset-manifest.json \
  --base out/base.gguf \
  --out out/base-eval.json

tools/adapter-training/evaluate.py \
  --test tools/adapter-training/data/test.jsonl \
  --dataset-manifest tools/adapter-training/data/dataset-manifest.json \
  --base out/base.gguf \
  --adapter out/adapter.gguf \
  --scale 0,0.5,1.0 \
  --grammar tools/adapter-training/proposal.gbnf \
  --mcp-grammar tools/adapter-training/mcp-tool-call.gbnf \
  --out out/adapter-eval.json
```

Use `--dev` to compare saved epoch checkpoints or adapter scales. Use `--test`
once for the release decision; model packaging rejects reports from dev.
Metrics report panel write selection/schema/refusal separately from MCP tool
selection/listing/schema/refusal, including unlisted MCP calls. Compare
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

## Historical TinyLlama run (2026-11-08)

This was an experimental run and is not the default model. TinyLlama is being
dropped because it did not reliably select tools or produce valid schemas.

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

The current release candidate is Qwen2.5-1.5B-Instruct with its Qwen-trained
adapter. Do not publish it until the MCP tool-name, schema-validity, refusal,
and existing panel-operation metrics have been measured against the base model
and the adapter, and the adapter materially improves the relevant scores.

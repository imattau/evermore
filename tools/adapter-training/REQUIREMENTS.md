# Evermore tool-use training requirements

This document defines the target for the next adapter experiment. It separates
tool execution from planning and follows the interfaces Evermore actually
ships. It does not authorize another training run or designate an adapter for
the VM image.

## Train the deployed tasks

Evermore currently has three distinct paths:

| Path | Model responsibility | Runtime responsibility |
| :--- | :--- | :--- |
| External MCP client | Choose one tool from the tools the host listed; provide its arguments or abstain. | MCP validates the call, checks the agent grant, and submits writes for owner approval. |
| `evermore-agentd propose` | Return one write proposal in the panel's marker format. | The daemon supplies only approval-gated write definitions, validates the proposal, and allows one repair attempt. It never submits or approves the write. |
| `evermore-agentd` recipes | No free-form planning is required by the current recipe runner. | Deterministic recipe steps perform declared reads and batch writes under grant, budget, and owner-approval checks. |

`evermore-mcp` is an MCP server, not a client or model tool loop. In the
current first-party runtime, `evermore-agentd` calls `inferd` only for its
single-write `propose` flow; it does not discover MCP tools, call them, or feed
tool results back to Qwen. Therefore the current MCP dataset evaluates an
external-host tool-call format, not an end-to-end local Evermore agent. If
local Qwen is expected to call MCP from inside Evermore, implementing and
evaluating that client loop is a prerequisite to claiming this capability.

Do not train the panel proposal task to select read operations: that runtime
accepts writes only. Do not train the model to plan recipe sequences or
operate `ai-contextd`, `ai-indexd`, or `inferd` directly. For MCP, the model
should use the live tool names, descriptions, and `inputSchema` supplied by
the host; the daemon names and socket protocols are implementation details.

Keep panel proposals and MCP calls as separately labeled tasks. A shared
adapter is acceptable only if held-out results show it does not damage either
path. A split selection/argument adapter is a candidate experiment, not an
assumption: compare it against a single adapter on Evermore's own test set
before adopting the extra runtime complexity.

## Training data

- Derive MCP tool availability and schemas from the shared catalogue and the
  exact MCP exposure rule. Include the MCP-only diagnostics separately. Bind
  every generated dataset to both its content hash and catalogue fingerprint.
- Train tool selection against realistic candidate lists and close
  alternatives. Include every current tool, no-tool cases, insufficient
  information, and explicit requests for approval-gated writes. A requested
  write remains a tool call; approval occurs after the call and is never a
  model decision.
- Use representative arguments that satisfy the real schema and domain
  rules. Cover required and optional fields, enums, formats, arrays, and
  cross-field constraints such as event end after start. Generic values such
  as `"example"`, one repeated timestamp, or one hash per operation are not
  sufficient coverage.
- Include wrong-tool and extra-field repairs, but do not make placeholder
  error fields the dominant repair pattern. Include semantically realistic
  user requests in language that does not name the target operation. Keep
  operation-name prompts as a smaller explicit-name slice.
- Build separate panel and MCP examples from their actual production prompts
  and output formats. Never reuse panel wording such as “propose this
  operation” as a proxy for an MCP user request.
- Keep human-reviewed, redacted traces opt-in. They must not contain private
  keys, credentials, or owner-only data.

Create three disjoint splits: train, development, and a locked final test.
Hold out paraphrases and task scenarios, not just alternate wordings of the
same generated catalogue description. Use the development split to select a
checkpoint. Do not use the final test split for checkpoint selection or prompt
tuning.

## Runtime and evaluation

The current GBNF files constrain output syntax, not a selected tool's exact
JSON schema. Training cannot replace runtime validation. The final evaluation
must measure both the raw model result and the production path, including
schema validation, the panel's one repair attempt, MCP rejection behavior,
grant checks, and owner approval for writes. An invalid or unlisted tool call
must never reach an operation side effect.

Report these separately; do not combine panel and MCP accuracy into one
operation score:

- MCP exact tool selection, including selection from close alternatives.
- MCP listed-tool rate and raw argument-schema validity.
- MCP end-to-end valid-call rate after the runtime validator, with invalid
  calls counted as failures if they reach the MCP server.
- Panel write-operation selection and schema validity through the real
  `agentd propose` prompt, grammar, validator, and repair loop.
- Unsupported-request abstention, clear-write submission rate, and policy
  violations. Report refusal of a supported write separately from refusal of
  an unsupported request.
- Base model, current best adapter, and candidate adapter on the same locked
  examples, base GGUF, tokenizer/chat template, grammar, context limit, and
  decoding settings.

The test set should contain at least five independently written positive
requests per exposed MCP tool, plus no-tool/ambiguous cases and approval-gated
write cases. Report counts and confidence intervals as well as percentages.
The current 110-case MCP slice is useful for diagnosis but too small and too
template-driven for release certification.

## Release gate

Keep the existing 95% selection and schema targets as minimums, but apply them
to separate MCP and panel metrics on the locked test set. Require a 100% listed
tool rate. Require zero unlisted calls, zero approval bypasses, and zero
unauthorized side effects in adversarial and integration checks. Unsupported
requests must meet at least 95% abstention accuracy without reducing the rate
of calling supported approval-gated writes. The candidate must meet the gates
and improve tool-use quality over both the base model and the current best
adapter without materially regressing the other path.

Select checkpoints by development-set generated tool-use metrics, not only
token-level `eval_loss`. Record the base, adapter, config, dataset, catalogue,
and final report hashes. Do not assemble the offline model store unless the
locked end-to-end test passes.

## Current evidence

The exploratory adapter v3 run (using dataset format v2) trained for 630 steps
and selected checkpoint 252 by development loss. On the then-current 110-case MCP slice, it scored 72.7% tool
accuracy and 70% correct-tool schema validity. The previous adapter scored
90% and 80% on that same slice. The full v3 run scored 84.9% combined
operation accuracy. These were exploratory results: the dataset had only
train/eval splits, and the same eval split was used for checkpoint selection
and final scoring. Neither result certifies a release.

The live catalogue has since grown to 39 MCP tools. Regenerate and verify the
tool inventory when defining the next train/dev/test set.

The current dataset generator is version 4: it emits separate train/dev/test
files, uses the runtime panel prompt helper, keeps panel write proposals
separate from MCP tool calls, and includes explicit argument values in each
positive request. It generates five varied positives per operation in each
split and validates samples against the live schemas. The requests and targets
are still catalogue-derived templates; the manifest records
`scenario_quality.reviewed: false`, and default-store packaging rejects it.
Treat this corpus as development data. Independently author and review the
locked test scenarios before another training run or any default-model
packaging.

## Exploratory v4 run (2026-11-09)

The Qwen2.5-1.5B run used the RTX 5060 Ti, 149 train records, 162 dev records,
four epochs, and 152 optimizer steps. Dev token loss fell to 0.0530, but the
VM's generated tool metrics did not select a new default. The previous best
adapter was stronger on MCP selection, listing, and schema validity.

Checkpoint comparison ran in the VM on the same 60-case balanced dev sample:
37 MCP operation positives, 14 panel write positives, and 9 refusals. The two
MCP diagnostic tools were absent from this dev sample. Percentages are raw
small-sample scores, not release evidence.

| Adapter | MCP select | MCP listed | MCP schema | Panel select | Panel schema | Panel refusal |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| Previous best | 97.3% | 97.3% | 83.8% | 100% | 71.4% | 100% |
| v4 epoch 38 | 94.6% | 97.3% | 78.4% | 92.9% | 78.6% | 40% |
| v4 epoch 76 | 91.9% | 94.6% | 67.6% | 85.7% | 57.1% | 100% |
| v4 epoch 114 | 89.2% | 91.9% | 70.3% | 92.9% | 64.3% | 100% |
| v4 epoch 152 | 89.2% | 91.9% | 70.3% | 100% | 64.3% | 100% |

No v4 checkpoint improves the prior adapter across both paths. The locked test
was not evaluated. Reports and the balanced sample are retained locally under
`.image-build/qwen-v4-eval/`; the PEFT checkpoints are under
`.image-build/qwen-v4-peft/`. Do not promote this run. The generator now pairs
requests with explicit target arguments, but independently author the
five-positive-per-tool test corpus before training again. The v4 run above used
the earlier v3 dataset format; its results do not evaluate the new corpus.

## Exploratory v5 run (2026-11-09)

At the user's request, an exploratory run used the RTX 5060 Ti and dataset
format v4: 310 train records, 325 dev records, four epochs, and 312 optimizer
steps. It took 11m15s; final dev token loss was 0.0524 and token accuracy was
0.9909. Training did not load `test.jsonl`. Dataset hash:
`e81fa12a381f67264a4622456cc6e4f0e1cb446de89b2af8f271f83bfb3ab962`.

All four epoch adapters were converted to F16 GGUF and evaluated in the VM
against the same 60-case balanced diagnostic sample used for the previous
adapter comparison. That sample is from dataset format v3, predates the v4
training data, and is only a comparative probe; do not use it to select a v5
checkpoint because overlap with the v5 training scenarios has not been ruled
out. The v4 dev split still needs to be evaluated in the VM for checkpoint
selection. Counts are 37 MCP tool cases, 14 panel writes, and 9 refusals; raw
scores are small-sample diagnostics.

| Adapter | MCP select | MCP listed | MCP schema | Panel select | Panel schema | Panel refusal |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| Base (scale 0) | 97.3% | 100% | 62.2% | 100% | 57.1% | 0% |
| Previous best | 97.3% | 97.3% | 83.8% | 100% | 71.4% | 100% |
| v5 epoch 78 | 94.6% | 97.3% | 83.8% | 100% | 64.3% | 40% |
| v5 epoch 156 | 94.6% | 94.6% | 75.7% | 92.9% | 64.3% | 100% |
| v5 epoch 234 | 100% | 100% | 70.3% | 100% | 57.1% | 100% |
| v5 epoch 312 | 100% | 100% | 73.0% | 100% | 71.4% | 100% |

Epoch 312 matches the previous best on panel metrics and improves MCP tool
selection/listing, but its MCP schema validity is lower (73.0% vs 83.8%). No
v5 checkpoint improves both paths. Keep v5 experimental; do not promote it or
evaluate the locked test until the independently authored scenario work is
done and the v4 development checkpoint comparison has been run.

## External benchmark evidence

Use external results as experiment ideas, not Evermore release targets. The
[DualTune paper](https://arxiv.org/abs/2510.00229) supports testing separate
selection and argument-generation stages, but reports its main result on
Qwen2.5-7B, so it does not establish that adapter separation will help this
1.5B model. The [NGARi model card](https://huggingface.co/NGARiAI/ngari-tool)
self-reports 100% on a 20-example tool-format evaluation. Another
[Qwen2.5-1.5B function-calling model card](https://huggingface.co/sagnik3788/Qwen2.5-1.5B-Instruct-FC)
reports 85.65% on BFCL v4 Non-Live AST using 10,000 verified synthetic
examples. Those datasets, tools, formats, and metrics differ from Evermore's
39-tool MCP catalogue; none substitutes for our locked end-to-end evaluation.

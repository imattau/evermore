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
| `evermore-agentd ask` | Choose a live MCP tool and valid arguments; use read results to decide whether another read is needed. | The daemon selects a separate MCP adapter, ranks the live catalogue to a six-tool shortlist, validates tool membership, and dispatches through MCP. MCP enforces schemas and grants; the loop stops after a write is submitted for owner approval. |
| `evermore-agentd` recipes | No free-form planning is required by the current recipe runner. | Deterministic recipe steps perform declared reads and batch writes under grant, budget, and owner-approval checks. |

`evermore-mcp` remains the authority-bearing server. The new `ask` path
discovers its live tools over stdio, gives Qwen a six-tool lexical shortlist,
and feeds successful read results back for up to two further tool calls. It
stops after any write request so the user can review the pending approval.
Keep evaluating the external MCP call format and the local orchestration path
separately; a correct tool choice alone does not prove a safe end-to-end run.

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

The test set should contain independently written positive requests for every
exposed MCP tool, plus no-tool/ambiguous cases and approval-gated write cases.
Five examples per tool is a useful coverage floor for finding obvious gaps, not
enough evidence to certify a per-tool success rate. Size the locked test for the
error bound needed by the release decision, and report counts and confidence
intervals as well as percentages. The current 110-case MCP slice is useful for
diagnosis but too small and too template-driven for release certification.

## Release gate

Do not use a blanket 95% raw selection or schema-validity score as a release
gate. Those are useful diagnostics, but the cutoff is not tied to user impact,
and a raw malformed proposal may be safely rejected or repaired by the
production path. Decide release on separate end-to-end outcomes for MCP and
panel: the requested supported task completes with the intended listed tool or
write operation, valid arguments, and the real validator/repair behavior. Set
the minimum acceptable completion rate and comparison margin against the
current best before opening the locked test, based on the intended use and the
cost of a failed or refused task. Report raw model scores, post-validation and
post-repair completion, false refusals of supported tasks, and unsupported
request abstention separately; a high score in one must not hide failures in
another.

Treat safety properties as hard gates: invalid arguments and unlisted tools
must be rejected before side effects; approval-gated writes must require owner
approval; and adversarial/integration checks must show no bypass or
unauthorized side effect. A zero-failure test result is evidence only for the
cases tested, so report the sample size and confidence bounds and expand the
locked suite according to the risk of the operation. Do not turn a small
sample's observed 100% into a claim of guaranteed safety.

Require every exposed tool and supported write operation to be represented in
the locked suite. Compare candidate, current best, and base on the same cases,
and do not promote a candidate that misses the predeclared end-to-end outcome
targets or materially regresses either MCP or panel use. Keep development-set
scores for checkpoint selection; do not tune against the locked test.

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
Treat this corpus as development data. After regeneration, run
`python3 tools/adapter-training/build-reviewed-test.py` to replace the test
split with the independently authored scenarios below. This leaves train/dev
unchanged and refreshes the manifest hashes.

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
out. The full v4 dev split has since been evaluated in the VM and is the valid
v5 checkpoint-selection result. It contains 325 records (232 MCP tool calls,
84 panel writes, and 9 refusals), with split hash
`c5890aa2f0b6f3119c224c3afd59fdb30d07ab87fa92e066bcb52c54aa877749`.

| Adapter | MCP select | MCP listed | MCP schema | Panel select | Panel schema | Panel refusal |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| Base (scale 0) | 97.3% | 100% | 62.2% | 100% | 57.1% | 0% |
| Previous best | 97.3% | 97.3% | 83.8% | 100% | 71.4% | 100% |
| v5 epoch 78 | 94.6% | 97.3% | 83.8% | 100% | 64.3% | 40% |
| v5 epoch 156 | 94.6% | 94.6% | 75.7% | 92.9% | 64.3% | 100% |
| v5 epoch 234 | 100% | 100% | 70.3% | 100% | 57.1% | 100% |
| v5 epoch 312 | 100% | 100% | 73.0% | 100% | 71.4% | 100% |

Those 60-case numbers are retained only to compare with the historical
previous-best run; the full-split table below should guide checkpoint
selection. Percentages are from all 325 v4 dev records:

| Adapter | MCP select | MCP listed | MCP schema | Panel select | Panel schema | Panel refusal | MCP refusal |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| v5 epoch 78 | 63.8% | 66.8% | 60.8% | 98.8% | 92.9% | 100% | 100% |
| v5 epoch 156 | 80.2% | 82.8% | 77.6% | 97.6% | 92.9% | 100% | 100% |
| v5 epoch 234 | 76.7% | 79.3% | 73.7% | 98.8% | 95.2% | 100% | 100% |
| v5 epoch 312 | 76.3% | 78.4% | 73.7% | 98.8% | 95.2% | 100% | 100% |

Epoch 156 is the strongest v5 checkpoint on MCP selection and schema metrics;
epochs 234 and 312 are stronger on panel schema validity. No checkpoint meets
the former 95% MCP and panel selection/schema targets across both paths. Keep
v5 experimental and do not promote it: these raw metrics do not establish
end-to-end outcomes, and the locked test remains unused.

## Reviewed locked test v1 (2026-11-09)

`tools/adapter-training/scenarios/locked-test-v1.jsonl` contains 289
independently authored cases: five positive requests for each of the 39 MCP
tools (195), five panel requests for each of the 14 approval-gated write
operations (70), and 24 unsupported-request refusals split across the MCP and
panel paths. The builder validates the live catalogue candidates, production
schemas, proposal marker, calendar constraints, and private-event participants;
it also checks unique IDs/requests and exact prompt disjointness from train
and dev. All cases carry `scenario_origin=independently-authored`.

The train and dev hashes remain
`2c691f9147482283bfbb7fd19d0a695a13afa31018f076d9e49259463a0571de` and
`c5890aa2f0b6f3119c224c3afd59fdb30d07ab87fa92e066bcb52c54aa877749`.
The locked test hash is
`9094fb664c59d6ea2665774228578bc20041de70e3c4cb8d28d592a5d4236093`;
scenario source hash is
`6e4317647ce604fc578c064a166e1fe25d743f795277e6413ed4150a0cae4186`.
The manifest records a Codex scenario and schema audit; this is not an
independent external human review. Preserve the test for a future checkpoint
that passes the development gates.

## Completion-only experiment v6 (2026-11-09)

The trainer now tokenizes each conversation as a prompt plus its final
assistant response and enables TRL `completion_only_loss`. The system prompt,
user request, and serialized MCP candidate list are masked from the objective;
all 310 training and 325 dev records retained at least one completion token.
The run used the same pinned Qwen2.5-1.5B base, v4 train/dev files, and four
epochs on the RTX 5060 Ti. It took 10m19s; train loss was 0.0122 and
completion-only dev loss was 0.0475.

The full 325-case VM evaluation was stopped at 131 cases after 11 minutes
because the VM is CPU-only and serial scoring was too slow. No full-split
report was produced. All four checkpoints were instead compared in the VM on
the same historical 60-case balanced dev diagnostic used for earlier
comparisons (37 MCP calls, 14 panel writes, 9 refusals). This v3 diagnostic is
comparative only and is not release evidence:

| Adapter | MCP select | MCP listed | MCP schema | Panel select | Panel schema | Panel refusal | MCP refusal |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| v6 epoch 78 | 89.2% | 89.2% | 64.9% | 92.9% | 64.3% | 100% | 100% |
| v6 epoch 156 | 97.3% | 100% | 70.3% | 100% | 71.4% | 100% | 100% |
| v6 epoch 234 | 94.6% | 94.6% | 67.6% | 100% | 78.6% | 100% | 100% |
| v6 epoch 312 | 100% | 100% | 73.0% | 100% | 85.7% | 100% | 100% |

Completion-only loss improved panel schema validity at later epochs but did
not resolve MCP argument quality. Failure inspection found repeated copying
of schema placeholders (`"sha256"`, `"path_component"`), extra fields copied
from neighboring tools, and requests that rely on an appended exact-JSON
argument hint. These diagnostics show the next training change should improve
the examples: use natural task scenarios and schema-valid, semantically
meaningful values, with fewer exact-JSON slot hints and less templated
description copying. Do not promote v6 or evaluate the locked test on it.

## Isolated-task v8 experiment (2026-11-09)

To check whether panel/MCP interference caused the v7 MCP regression, v8
trained separate adapters from the same v5 corpus with completion-only loss.
Both were evaluated in the VM with the release `llama-server` on a fixed
current-dev diagnostic: 39 MCP calls plus 4 MCP refusals, and 14 panel writes
plus 5 panel refusals. These are checkpoint-selection samples only.

| Adapter | Epoch/step | MCP listed + valid schema | Panel operation | Panel schema | Refusal |
| :--- | ---: | ---: | ---: | ---: | ---: |
| MCP-only | 102 | 15.4% (6/39) | — | — | 100% |
| MCP-only | 204 | 89.7% (35/39) | — | — | 100% |
| MCP-only | 306 | 89.7% (35/39) | — | — | 100% |
| MCP-only | 408 | 87.2% (34/39) | — | — | 100% |
| Panel-only | 40, 80, 120, 160 | — | 100% (14/14) | 92.9% (13/14) | 100% |

MCP's remaining failures were false refusals for supported index, settings,
calendar, and notification tools. Panel's remaining schema failure added an
unsupported `visibility` property to `calendar.update_event`. Do not promote
v8; the locked test was not evaluated. Reports are retained locally under
`.image-build/qwen-v8-eval/`.

## Targeted augmentation v9 (2026-11-09)

After v8, six natural phrasings were added as train-only MCP positives for
the tools that had been falsely refused; one calendar `visibility` repair was
also added. The authored locked-test hash stayed
`e3cb6a305d4ed2a1c3d45c9c05344e134b3bfc0b7027b0123a2c035001194f27`. Separate
adapters trained on the revised v6 corpus. The same fixed dev diagnostics were
scored inside the VM:

| Adapter | Epoch/step | MCP correct + valid schema | MCP refusal | Panel selected | Panel schema |
| :--- | ---: | ---: | ---: | ---: | ---: |
| MCP-only | 104 | 94.9% (37/39) | 100% (4/4) | — | — |
| MCP-only | 208 | 100% (39/39) | 100% (4/4) | — | — |
| MCP-only | 312 | 100% (39/39) | 100% (4/4) | — | — |
| MCP-only | 416 | 100% (39/39) | 100% (4/4) | — | — |
| Panel-only | 40, 120, 160 | — | — | 100% (14/14) | 92.9% (13/14) |
| Panel-only | 80 | — | — | 100% (14/14) | 78.6% (11/14) |

MCP v9 replaces the earlier false refusals on those six operations, but two
different MCP requests failed at step 104; later epochs passed the fixed
39-call diagnostic. Panel v9 still adds `visibility` to
`calendar.update_event`. These small, held-out diagnostics are not release
certification and do not establish quality across the complete dev or locked
test sets. Do not promote either adapter yet. Reports are retained locally
under `.image-build/qwen-v9-eval/`.

## Panel privacy augmentation v10 (2026-11-09)

Four train-only calendar update phrasings explicitly asked to preserve
existing privacy and recipient settings. The panel-only adapter trained for
four epochs on v7. Every VM checkpoint passed the fixed 14-write/5-refusal
diagnostic: 14/14 operations selected, 14/14 argument schemas valid, and 5/5
refusals correct. Reports are under `.image-build/qwen-v10-eval/`. This is a
promising panel checkpoint, but the sample is small and not release evidence;
the locked test remains unused.

## Combined-task v11 experiment (2026-11-09)

Because the current image carries one adapter, a combined run on the v7 corpus
tested whether those specialized results could coexist in a single LoRA.
On the same fixed 62-case VM diagnostic (39 MCP calls, 14 panel writes, 9
refusals), step 144 scored MCP 1/39 and panel 14/14 schema-valid; step 288
scored MCP 2/39 and panel 14/14; step 576 scored MCP 7/39 and panel 14/14.
Refusals were 9/9 at every checkpoint. The locked test remains unused. These
results indicate strong task interference in the combined adapter. Reports
are retained under `.image-build/qwen-v11-eval/`; do not promote it.

## Full task-specific VM development evaluation (2026-11-09)

After adding separate model routing and the local MCP client loop, both
selected adapters were run against every record for their task in the
generated development split. These raw-generation metrics are separate from
the successful one-call runtime smoke check.

| Adapter | Dev records | Tool/operation selection | Listed MCP tool | Schema-valid | Refusal |
| :--- | ---: | ---: | ---: | ---: | ---: |
| MCP v9 epoch 208 | 422 (418 calls, 4 refusals) | 410/418 (98.1%) | 410/418 (98.1%) | 396/418 (94.7%) | 4/4 (100%) |
| Panel v10 epoch 41 | 159 (154 writes, 5 refusals) | 151/154 (98.1%) | — | 136/154 (88.3%) | 5/5 (100%) |

Both are below the former 95% schema target. MCP errors still include extra
package/model fields such as `sha256` and `type`; panel errors include extra fields on
`ai.model.install_signed_npack`, `notifications.set_dnd`, and
`packages.remove`, plus malformed or overlong `calendar.update_event` output.
Do not run the locked test or bundle these candidates. The local runtime smoke
check did pass panel routing and a real read-only `mcp_status` call through the
MCP stdio server. Full reports are retained in the ignored local directory
`.image-build/full-dev-eval/`.

## Retraining regression audit (2026-11-10)

The v9 expanded train split (1,236 records) regressed MCP full-dev tool
accuracy to 360/418 (86.1%) and correct MCP schemas to 346/418 (82.8%). Its
panel schema score was 135/154 (87.7%). The main MCP failure mode was
unnecessary refusal on listed tools. Do not use v9 adapters.

The v10 train split was restored to 848 records and kept the v8 dev and locked
test file hashes unchanged. Full VM task-dev results were 17/418 MCP tool
choices and 17/418 valid schemas (4.1%); all 4 MCP refusals were correct. The
panel adapter reached 142/154 valid schemas (92.2%), with 153/154 operation
choices and all 5/5 panel refusals correct. The v10 halfway MCP checkpoint
also scored only 2/80 tool choices on the focused VM diagnostic.

As a control, the v8 step-290 MCP adapter scored 77/80 tool choices (96.3%)
and 64/80 valid schemas (80.0%) on the same focused cases. On the full MCP
task-dev subset it scored 406/418 tool choices (97.1%), 388/418 valid schemas
(92.8%), and 4/4 MCP refusals. This confirms the VM path is functioning and
that v9/v10 selection regressions are training regressions. V8 is the strongest
current adapter on these raw development metrics. Keep the locked test unused
and do not bundle any adapter until the production-path outcomes and safety
properties in the release gate have been evaluated. The raw schema results
alone neither qualify nor disqualify it.

## v14 full development evaluation (2026-11-10)

The v14 adapters were evaluated in the VM with the release `llama-server` on
the full task-specific development cases, using the MCP tool-call grammar or
panel proposal grammar as appropriate. The MCP set had 418 supported calls
and 4 expected refusals; the panel set had 154 writes and 5 expected refusals.
This is raw model output plus grammar and schema scoring, not the production
validator/repair path.

| Adapter | Cases | Correct/listed tool or operation | Schema-valid output | Correct refusals |
| :--- | ---: | ---: | ---: | ---: |
| v14 MCP final | 418 calls + 4 refusals | 17/418 (4.1%; 95% Wilson CI 2.6–6.4%) | 17/418 (4.1%; 2.6–6.4%) | 3/4 (75%) |
| v14 MCP step 290 | 418 calls + 4 refusals | 44/418 (10.5%; 7.9–13.8%) | 43/418 (10.3%; 7.7–13.6%) | 3/4 (75%) |
| v14 panel final | 154 writes + 5 refusals | 153/154 (99.4%; 96.4–99.9%) | 142/154 (92.2%; 86.9–95.5%; checked against expected operation schema) | 5/5 (100%) |

On these same development requests, the previous MCP v8 checkpoint scored
406/418 tool selections and 388/418 correct-tool schemas; the previous panel
v10 checkpoint scored 142/154 valid schemas. V14 regresses sharply on MCP and
does not improve panel schema validity. Do not promote these v14 adapters.

As a narrow production-path smoke check, the v14 panel produced a validated
`settings.set` proposal and each MCP checkpoint completed one read-only status
request through `agentd ask` and the real MCP stdio server. These one-request
smokes do not estimate end-to-end task completion or exercise invalid-call
rejection, repair recovery, or approval handling across the evaluation set.
The raw evaluator's `mcp_unlisted_calls` counter also treats a refusal as a
non-listed prediction; do not interpret that counter as a count of calls that
reached MCP or caused side effects.

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

# evermore-agentd

Deterministic recipe runner for Evermore's bounded agent (Phase 11). It groups
existing catalogue operations into durable tasks; it never executes a system
operation itself and never holds execution authority.

## What it does

- Loads a built-in `audit.digest` recipe and validated local TOML recipes from
  `$XDG_DATA_HOME/evermore/recipes`.
- Persists tasks in `$XDG_DATA_HOME/evermore/tasks/tasks.json` with private
  permissions (`0700` directory, `0600` file, atomic replace).
- Runs catalogue reads through `evermore-operationsd`'s Unix socket and reads
  the private operation journal for local views.
- Reuses the existing signed `kind:1` operation request, adding `task` and
  `step` tags so the panel can group a step with its task. There is no second
  lifecycle.
- Submits approval-gated write steps as ordinary signed operation requests,
  batching all writes in a recipe so the owner reviews the whole plan together.
  Each write is still approved by its own exact owner signature.
- Marks a task `execution_unknown` after a restart if a step was claimed, and
  never replays it. Tasks waiting for approval resume when the operation
  reaches a terminal state.

## Recipe format

Local recipes live in `$XDG_DATA_HOME/evermore/recipes/*.toml`, must be regular
files with no group/world write permission (`chmod 600`), and are capped at
256 KiB. Unknown operations, undeclared operation use, forward template
references, a write step followed by a non-write step, a write beyond the write
budget, oversized step counts, and budgets over 32 steps or 300 seconds are
refused.

```toml
id = "workspace.open"
version = 1
title = "Open a workspace"
description = "Fill a workspace number and render a summary."
operations = []                       # catalogue reads this recipe may call
budget = { steps = 3, writes = 0, seconds = 15, context_tokens = 0 }

[[step]]
id = "choose"
kind = "select"
schema = { type = "object", properties = { workspace = { type = "integer", minimum = 1, maximum = 9 } }, required = ["workspace"] }

[[step]]
id = "summary"
kind = "journal"                      # pending | history | summary
view = "summary"

[[step]]
id = "text"
kind = "render"
template = "Workspace {{choose.workspace}}. Pending requests: {{summary.pending}}."
```

Step kinds in v1:

| Kind | Purpose |
| :--- | :--- |
| `select` | Declare typed inputs the panel renders as a form. |
| `read` | Invoke one declared read operation through `evermore-operationsd`. Requires a matching owner grant. |
| `journal` | Read the private operation journal (`pending`, `history`, `summary`). |
| `render` | Deterministically interpolate earlier step values with `{{step.field}}`. |
| `write` | Submit one approval-gated operation. Writes must be the final steps so the owner reviews the whole write plan before anything runs. |
| `each` | Iterate an array from an earlier read step, render one approval-gated write per item, and submit them together. `each` must be the final step and is capped by the write budget. |

## Usage

```sh
evermore-agentd recipes
evermore-agentd run audit.digest
evermore-agentd propose "switch the theme to dark"
evermore-agentd ask "what is my current workspace?"
evermore-agentd tasks
evermore-agentd status TASK_ID
evermore-agentd cancel TASK_ID
```

## Standing routines

Routines are explicit, per-entry standing tasks. Each runs as a dedicated agent
identity, must use a distinct identity from both the owner and the interactive
assistant, and is checked against that identity's grant when added and again
when each run starts. There is no general "background agent" switch.

```sh
evermore-agentd routines
evermore-agentd routine-add daily-digest audit.digest \
  --interval 86400 --agent-identity routine-digest
evermore-agentd routine-disable daily-digest
evermore-agentd routine-remove daily-digest
```

The daemon persists routines in
`$XDG_DATA_HOME/evermore/routines/routines.json` (0600) and runs a due routine
at most once per interval. A routine task records its `routine_id`, so the
panel's Recent Activity shows which standing identity created it. Write
routines still stop at `awaiting_approval`; the owner approves each exact
operation. A missing or expired grant is recorded as the routine's
`last_error` and no task starts.

`propose` is the L1 generation path: it asks the local chat model for one
grammar-constrained operation proposal, validates it against the catalogue,
repairs malformed or out-of-catalogue output at most once, and returns the
canonical arguments without submitting anything. The panel's "Describe the
change" field uses the same request and fills the existing owner-approval form.
The proposal path enforces a 4096-token context budget; task recipes carry the
same field as `budget.context_tokens` for future model steps.

`ask` runs a local MCP tool conversation as the identity configured for
the running `evermore-agentd serve` process. The service must be running for
the CLI request to reach it. It starts `evermore-mcp` over stdio, gets the live
catalogue, selects the installed MCP adapter, and rejects unlisted tool names
before dispatch. MCP still enforces the exact operation grant and validates
arguments. Read results can guide up to two further calls; after a write is
submitted, the conversation stops and reports the owner-approval-pending
result. Set `EVERMORE_MCP_COMMAND` in the daemon's service environment only
when the MCP executable is installed outside the standard PATH, and
`EVERMORE_MCP_MODEL` there to override the installed MCP model name. Panel
proposal routing can be overridden with `EVERMORE_PANEL_MODEL`.

Built-in recipes include `audit.digest`, `packages.doctor`,
`packages.updates`, `settings.concierge`, `workspace.compose`, and
`notifications.dnd`. The write recipes submit all their writes together, so the
panel shows one pending-write plan with canonical arguments; "Approve all"
signs one owner approval per exact operation.

`packages.updates` is the update planner: it reads `packages.update_plan`
(trusted-publisher check plus exact version and SHA-256 resolution), then its
`each` step submits one `packages.install_pinned` request per available update.
Resolver failures and revoked releases are reported separately; a package that
fails or is denied does not stop the others, and the task ends
`partially_completed` rather than retrying. Zero updates completes without any
approval request.

The daemon needs a loaded signer identity for its agent pubkey and for signing
write requests:

```sh
evermore-agentd serve --agent-identity NAME [--owner-pubkey HEX]
```

`packaging/systemd/evermore-agentd@.service` runs it as `%i-assistant`. It has
no shell, no process-spawn operation, and no direct write path.

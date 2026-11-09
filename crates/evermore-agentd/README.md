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
- Refuses write steps and non-zero write budgets in this version. Writes remain
  ordinary owner-approved operations when M11.3 lands.
- Marks a task `execution_unknown` after a restart if a step was claimed, and
  never replays it.

## Recipe format

Local recipes live in `$XDG_DATA_HOME/evermore/recipes/*.toml`, must be regular
files with no group/world write permission (`chmod 600`), and are capped at
256 KiB. Unknown operations, undeclared operation use, forward template
references, write steps, oversized step counts, and budgets over 32 steps or
300 seconds are refused.

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

## Usage

```sh
evermore-agentd recipes
evermore-agentd run audit.digest
evermore-agentd propose "switch the theme to dark"
evermore-agentd tasks
evermore-agentd status TASK_ID
evermore-agentd cancel TASK_ID
```

`propose` is the L1 generation path: it asks the local chat model for one
grammar-constrained operation proposal, validates it against the catalogue,
repairs malformed or out-of-catalogue output at most once, and returns the
canonical arguments without submitting anything. The panel's "Describe the
change" field uses the same request and fills the existing owner-approval form.
The proposal path enforces a 4096-token context budget; task recipes carry the
same field as `budget.context_tokens` for future model steps.

The daemon needs a loaded signer identity for its agent pubkey:

```sh
evermore-agentd serve --agent-identity NAME [--owner-pubkey HEX]
```

`packaging/systemd/evermore-agentd@.service` runs it as `%i-assistant`. It has
no shell, no process-spawn operation, and no direct write path.

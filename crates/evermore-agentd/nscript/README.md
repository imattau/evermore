# M12.3/M12.4: evermore-agentd recipes authored as nscript

Status: **wired in, and proven on a genuinely new recipe.** `evermore-agentd` depends on `nscript`'s own
`nscript-syntax`/`nscript-semantics`/`nscript-modules` library crates (pinned
to an exact commit on the `declared-input` branch — see
`crates/evermore-agentd/Cargo.toml`) and projects a checked `.ns` program
directly into the existing `Recipe`/`RecipeStep` structs in
[`crates/evermore-agentd/src/nscript_recipe.rs`](../src/nscript_recipe.rs).
`evermore-agentd`'s recipe loader (`recipe::load_all`/`load_local`) now
accepts `.ns` files alongside `.toml`, and every projected recipe goes
through the exact same `validate_recipe` a hand-written TOML recipe does —
see [`PROJECT_IDEA.md`](../../../PROJECT_IDEA.md)'s Phase 12 addendum for why
that single enforcement point matters.

**`nscript`'s own runtime, `OperationHost`, and simulator never run inside
`evermore-agentd`.** The projector only uses `nscript`'s parser and checker
as a typed front end — the same way any compiler uses a parser library
without running the program it parses. Execution, grant checks, and the
owner-approval path are entirely `evermore-agentd`'s own, unchanged.

**Every bundled `.nsm` module's own operation signatures are a second,
hand-maintained copy of the real catalogue's.** Nothing about `nscript`'s
own checker catches the two drifting apart — it only validates a call
against the module's *own* declared signature, never against
`evermore_common::operations::catalog()`. `nscript_recipe::tests::bundled_module_signatures_match_the_real_catalogue`
cross-checks every bundled module's declared parameter names, counts, and
JSON-schema-equivalent types against the real catalogue entry each
operation claims to stand for, so a future change to either side that
silently diverges fails CI instead of surfacing as a confusing recipe
validation error (or, worse, a wrong one that still validates).

## What this directory contains

- `modules/evermore_packages/0.1.0.nsm` — a **non-NIP**, Evermore-specific
  `nscript` module declaring two real catalogue operations
  (`evermore_common::operations::catalog()`), `list_installed` and
  `explain_dependencies`, as typed `nscript` operations. Bundled into the
  `evermore-agentd` binary at compile time via `include_str!`
  (`nscript_recipe::evermore_module_registry`) — the daemon never reads it
  from disk at run time.
- `recipes/packages.doctor.ns` — the full built-in `packages.doctor` recipe
  ([`../recipes/packages.doctor.toml`](../recipes/packages.doctor.toml)),
  authored directly in `nscript`. A test
  (`nscript_recipe::tests::packages_doctor_ns_projects_onto_the_hand_written_toml_recipe`)
  asserts the projected `Recipe` matches the hand-written TOML one
  field-for-field — same `id`/`title`/`description`/`operations`/`budget`,
  same four step ids in the same order, same `select` schema, same `read`
  args, same `render` template. This only ever exercised the projector's
  **read + render** path, because it mirrors a recipe that already existed
  as hand-written TOML.
- `modules/evermore_notifications/0.1.0.nsm` and
  `recipes/notifications.triage.ns` — **M12.4's genuinely new recipe**, with
  no hand-written TOML counterpart: review recent notifications
  (`notifications.list_recent`, a read), preview which one is about to be
  dismissed, then dismiss it by local ID (`notifications.dismiss`, a write
  owner approval already gates — see `evermore_common::operations::catalog()`).
  This is the first recipe to exercise the projector's **write** path:
  `select` → `read` → `render` → `write`. Asserted by
  `nscript_recipe::tests::notifications_triage_ns_projects_a_read_then_a_render_then_a_write_step`.
- `modules/evermore_ai/0.1.0.nsm` — the AI status/list subset:
  `infer_status`, `model_list`, `index_status`, and `context_status`. The
  catalogue nests these (`ai.infer.status`), so the projector maps the short
  nscript names explicitly rather than deriving `ai.<name>` from the module.
- `recipes/desktop.check.ns`, `recipes/updates.watch.ns`, and
  `recipes/ai.check.ns` — the first **default agent templates**, all
  read-only. They are loaded as built-ins and paired with
  `templates/default-agents.toml`, which supplies the gallery name, summary,
  cadence, expiry, and manual/scheduled default.
- The module signature test now also compares the **return shape** (array vs
  object) against the real catalogue. This is what caught
  `notifications.list_recent` and `packages.list_installed` declaring records
  while the catalogue returns arrays.

## The conventions the projector requires

`nscript` has no concept of several things a recipe needs, so the projector
fixes a convention for each rather than guessing:

- **Recipe id.** Always the file name without `.ns` (`packages.doctor.ns` →
  `packages.doctor`).
- **Title and description.** `nscript` has no such metadata and discards
  comments entirely during parsing, so the projector reads two leading
  `// title: ...` / `// description: ...` lines directly from the raw source
  text, in either order, before parsing. A recipe without a `// title: ...`
  line is refused.
- **Module naming.** `use evermore_<prefix>` resolves to the real catalogue
  operation prefix `<prefix>` (`evermore_packages` → `packages.*`). The AI
  namespace is explicitly mapped because its catalogue names are three
  segments: `evermore_ai.infer_status` → `ai.infer.status`,
  `model_list` → `ai.model.list`, `index_status` → `ai.index.status`, and
  `context_status` → `ai.context.status`. A module that doesn't map to a real
  catalogue operation is refused rather than silently skipped.
- **Shape.** Zero or more `input name: Type` declarations (→ one `select`
  step, id `choose`); exactly one `every`/`at` block whose body is zero or
  more `let name = module.op(args)?` statements (each → one `read` or
  `write` step, step id = the `let` name, kind decided by looking the real
  operation up in `evermore_common::operations` — never by the script) plus
  at most one `print(...)` (→ one `render` step, id `report`, built from the
  `+`-concatenated literal/`text(...)`/field-reference parts), **projected
  at its own position in the schedule body** rather than always last. Since
  `crate::recipe::validate_recipe` requires `write`/`each` to be a recipe's
  own final step, a `print(...)` written before a later write previews
  already-known values (M12.4's `notifications.triage.ns` does this); one
  written after a write is refused the same way a hand-written TOML
  recipe's would be — `render` cannot follow a write either way, so a
  write-containing recipe's only choice is to put its `print(...)` earlier
  or omit it. Anything else in the schedule body — an `if`, a `for`, a
  second `print`, a second schedule — is refused with a specific message.
- **Operation arguments are positional**, matching the bundled `.nsm`
  module's own declared parameter order (read from the resolved module
  graph, not the catalogue's `input_schema`, whose JSON key order is not
  guaranteed to match either). A bare identifier argument must name a
  declared `input`; anything else is refused.
- **A write-containing recipe can `render`, but only to preview, never to
  summarise.** This was a real projector bug, now fixed: the projector used
  to always append the `render` step last regardless of where `print(...)`
  appeared in the source, so any schedule body with both a write and a
  `print(...)` failed `crate::recipe::validate_recipe`'s "write/each must
  be a recipe's final step" rule no matter where the `print(...)` was
  written. The projector now places `render` at its own position, so a
  `print(...)` before the write (as in `notifications.triage.ns`, M12.4)
  projects and validates. What's left is `validate_recipe`'s own rule, not
  a projector bug: a `print(...)` *after* the write is still refused,
  because `render` is not itself a `write`/`each` step and so cannot follow
  one. A write-heavy recipe that wants to report what it actually did,
  rather than what it's about to do, needs a different mechanism than
  `print(...)` — this projector doesn't invent one.

## A real, non-obvious finding along the way

`nscript` string literals do not interpret `\n` as an escape — the lexer
only tracks a `\` to know a following `"` is not the string's end; the
stored value is the raw source substring either way. A `print(...)` whose
template needs a line break must put an actual newline character inside the
quotes (`"foo` then a literal line break then `bar"`), not `"\n"`, or the
projected `render` step's template would contain a literal backslash-n
instead of a line break. `packages.doctor.ns` does this; the projector's own
test caught the literal-`\n` version before it reached this README.

## What came before this

Two earlier gaps blocked this from working end to end, both closed in this
same change set:

1. **No native "typed form input" primitive** — closed by
   [`imattau/nscript#12`](https://github.com/imattau/nscript/pull/12),
   `input name: Type`.
2. **No CLI-level way to extract a program's statement-by-statement
   structure** (`nscript inspect --json` and `nscript compile --emit ir`
   both stop at a summary) — this is exactly why the projector depends on
   `nscript`'s library crates directly and walks the checked AST in Rust,
   rather than shelling out to the built `nscript` binary.

`nscript`'s built-in simulator still cannot fake a custom module's return
shape (so `nscript run packages.doctor.ns` itself still fails past the
`list_installed` call) — but the real projector never runs that simulator,
so this does not block `evermore-agentd`'s own use of the recipe.

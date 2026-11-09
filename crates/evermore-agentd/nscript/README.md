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
  (`notifications.list_recent`, a read), then dismiss one by its local ID
  (`notifications.dismiss`, a write owner approval already gates — see
  `evermore_common::operations::catalog()`). This is the first recipe to
  exercise the projector's **write** path: `select` → `read` → `write`, no
  `render` (`validate_recipe` requires write steps to be a recipe's final
  steps, so a write-containing recipe cannot also render a summary — this
  rules out the shape the earlier read-only prototype used). Asserted by
  `nscript_recipe::tests::notifications_triage_ns_projects_a_read_then_a_write_step`.

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
  operation prefix `<prefix>` (`evermore_packages` → `packages.*`). A module
  that doesn't follow this is refused rather than silently skipped.
- **Shape.** Zero or more `input name: Type` declarations (→ one `select`
  step, id `choose`); exactly one `every`/`at` block whose body is zero or
  more `let name = module.op(args)?` statements (each → one `read` or
  `write` step, step id = the `let` name, kind decided by looking the real
  operation up in `evermore_common::operations` — never by the script) plus
  at most one `print(...)` (→ one `render` step, id `report`, built from the
  `+`-concatenated literal/`text(...)`/field-reference parts). Anything else
  in the schedule body — an `if`, a `for`, a second `print`, a second
  schedule — is refused with a specific message.
- **Operation arguments are positional**, matching the bundled `.nsm`
  module's own declared parameter order (read from the resolved module
  graph, not the catalogue's `input_schema`, whose JSON key order is not
  guaranteed to match either). A bare identifier argument must name a
  declared `input`; anything else is refused.
- **A write-containing recipe cannot also `render`.** The projector always
  appends the `render` step last (if a `print(...)` is present), regardless
  of where it appears in the source relative to the schedule body's `let`
  statements; `crate::recipe::validate_recipe` separately requires every
  `write`/`each` step to be a recipe's final step. Put the two together and
  a schedule body with both a write operation call and a `print(...)` will
  always fail validation — the `render` step lands after the write either
  way. `notifications.triage.ns` (M12.4) is `select` → `read` → `write`
  with no `print(...)` for exactly this reason.

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

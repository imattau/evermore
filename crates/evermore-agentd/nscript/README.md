# M12.3 prototype: can an evermore-agentd recipe be authored as nscript?

Status: validated at the type-checking layer, including a real `--input`
end-to-end run. **Nothing here is wired into `evermore-agentd`.** No
`evermore-agentd` Rust code changed; `recipe.rs`'s loader still only reads
`.toml`. See [`PROJECT_IDEA.md`](../../../PROJECT_IDEA.md)'s Phase 12
addendum for the design this is prototyping.

Per that design, `nscript` is meant to be a typed *authoring front end* for
recipes, never a runtime inside `evermore-agentd` — the checked program gets
projected into the existing `Recipe`/`RecipeStep` structs, which
`evermore-agentd`'s loader validates and executes exactly as it does a
hand-written TOML recipe today.

## What this directory contains

- `modules/evermore_packages/0.1.0.nsm` — a **non-NIP**, Evermore-specific
  `nscript` module declaring two real catalogue operations
  (`evermore_common::operations::catalog()`): `packages.list_installed` and
  `packages.explain_dependencies`, as typed `nscript` operations.
- `recipes/packages.doctor.ns` — the **full** built-in `packages.doctor`
  recipe ([`../recipes/packages.doctor.toml`](../recipes/packages.doctor.toml)),
  including its `choose` form (now `input publisher: Text` / `input name:
  Text`), both reads, and a rendered report.

Both type-check and run cleanly against a real `nscript` build (`nscript`
PR [#12](https://github.com/imattau/nscript/pull/12), not yet merged at the
time of writing — see "What changed upstream" below):

```
$ nscript check -M modules recipes/packages.doctor.ns
(no output: clean)
$ nscript run -M modules recipes/packages.doctor.ns \
    --input publisher=abc123 --input name=example-pkg
timer schedule-0 at ...
error[R1002]: EvaluationError { message: "a text has no fields" }
```

That last error is expected and explained below — it is the *other* open
gap this prototype found, not a failure of `input`. A smoke test with only
scalar prints (no custom-module field access) runs clean end to end; see
"What this proves" below.

## What changed upstream to get here

The first version of this prototype (see git history) shipped without
`packages.doctor`'s `choose` step, because `nscript` had no primitive for
"a typed value the host collects before running." That gap is now closed
upstream: [`imattau/nscript#12`](https://github.com/imattau/nscript/pull/12)
adds `input name: Type`, bound into schedule-body scope exactly like a
`let`, with `nscript run`/`deploy --input <name>=<value>` supplying it. The
recipe here uses it directly (`input publisher: Text`, `input name: Text`),
and `nscript run --input publisher=... --input name=...` correctly binds
and uses both.

## What this proves

- `nscript`'s module system works as a typed front end for Evermore-specific,
  non-NIP operations, both read-only (`list_installed`) and parameterised
  (`explain_dependencies(publisher, name)`), not just NIP-shaped ones.
- `input` closes the "typed form" gap completely for scalar fields: a smoke
  test using only `Text`/`Int`/`Bool` inputs and plain `print` statements
  (no custom-module field access) runs end to end through `nscript run
  --input ...` with the exact supplied values. `nscript`'s fixed effect
  vocabulary (`Relay`, `Sign`, `Encrypt`, `Decrypt`, `Storage`, `HTTP`,
  `Filesystem`, `Payment`, `Clock`, `Log`) still has a reasonable-enough fit
  for a local, no-grant-check read (`Storage`), as the first version of this
  prototype found.

## What this still does not prove, and the remaining open gap

**`nscript`'s built-in simulator (`run`'s default) cannot fake a custom
module's return shape.** `SimulatedOperations` answers operations it does
not have a real host for by returning a generic placeholder (`Text("simulated")`)
built from the few `OperationValue` variants the crate ships concrete Rust
structs for (NIP-shaped ones: `UserList`, `AppData`, `PublishReport`, ...).
`evermore_packages.list_installed`/`explain_dependencies` return
Evermore-specific records (`PackageListing`, `DependencyExplanation`) with
no such built-in variant, so the simulator's placeholder has no `count` or
`version` field — hence the `"a text has no fields"` error above when the
recipe accesses them. This is orthogonal to `input` (which worked correctly
right up to that point) and to the already-merged
[`ProcessOperationHost`](https://github.com/imattau/nscript/pull/9): that
feature's own `decode()` only recognises a fixed whitelist of NIP-shaped
record names too, not arbitrary custom ones, so pointing
`--operation-command` at a real backend wouldn't resolve this either without
extending that whitelist.

The real `nscript → Recipe` projector (the part of M12.3 that would depend
on `nscript`'s library crates directly, not the CLI) sidesteps this
entirely: it would walk the checked AST and build `evermore-agentd`'s own
`Recipe`/`RecipeStep` values, never running `nscript`'s evaluator or
simulator at all. This gap only matters for *exercising* a prototype
recipe with `nscript run`/`deploy` directly, which is useful for authoring
and validation but is not how the eventual integration would execute
anything.

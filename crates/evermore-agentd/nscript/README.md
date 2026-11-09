# M12.3 prototype: can an evermore-agentd recipe be authored as nscript?

Status: validated at the type-checking layer only. **Nothing here is wired
into `evermore-agentd`.** No Rust code changed; `recipe.rs`'s loader still
only reads `.toml`. See [`PROJECT_IDEA.md`](../../../PROJECT_IDEA.md)'s
Phase 12 addendum for the design this is prototyping.

Per that design, `nscript` is meant to be a typed *authoring front end* for
recipes, never a runtime inside `evermore-agentd` — the checked program gets
projected into the existing `Recipe`/`RecipeStep` structs, which
`evermore-agentd`'s loader validates and executes exactly as it does a
hand-written TOML recipe today.

## What this directory contains

- `modules/evermore_packages/0.1.0.nsm` — a **non-NIP**, Evermore-specific
  `nscript` module declaring one real catalogue operation,
  `packages.list_installed` (`evermore_common::operations::catalog()`), as a
  typed `nscript` operation (`PackageListing { count: Integer }`).
- `recipes/packages.doctor.ns` — a prototype recipe using it: one read, one
  rendered line. It deliberately covers only the read-only half of the
  built-in `packages.doctor` recipe
  ([`../recipes/packages.doctor.toml`](../recipes/packages.doctor.toml)).

Both type-check and simulate cleanly against a real `nscript` build:

```
$ nscript check -M modules recipes/packages.doctor.ns
(no output: clean)
$ nscript run -M modules recipes/packages.doctor.ns
operation 0: Text("simulated")
```

## What this proves

- `nscript`'s module system works as a typed front end for an
  Evermore-specific, non-NIP read operation, not just NIP-shaped ones. Its
  checker produces exactly the information a projector would need: the
  `inspect --json` output lists the resolved operation
  (`evermore_packages.list_installed`), its permission
  (`packages_read`), and its capability group (`Storage`) — the same shape
  `evermore-agentd`'s own `operations`/grant model already uses.
- `nscript`'s fixed effect vocabulary (`Relay`, `Sign`, `Encrypt`, `Decrypt`,
  `Storage`, `HTTP`, `Filesystem`, `Payment`, `Clock`, `Log`) has a
  reasonable-enough fit for a local, no-grant-check read: `Storage` is the
  closest available effect, used here for "read Evermore's own catalogue,"
  which is not really storage I/O but isn't any of the others either. This
  is a labelling compromise, not a semantic mismatch worth blocking on.

## What this does not prove, and the two open gaps it found

1. **No native "typed form input" primitive.** `packages.doctor`'s first
   step (`choose`, a `publisher`/`name` form the owner fills in before the
   read runs) has no nscript equivalent. `key = host("label")` is typed for
   opaque signing-key material, not reusable for arbitrary form data, and
   nscript has no other "a value the host collects before running"
   construct. This prototype's recipe skips that step entirely rather than
   force-fit it — a real compiler needs an answer here before it can cover
   any recipe with a `select` step, which is most of the built-in set.
2. **No CLI-level extraction of step-by-step structure.** Neither
   `nscript inspect --json` (a summary: resolved operations, permission
   footprint, counts) nor `nscript compile --emit ir` (a coarser
   manifest-level capability/permission list, `operations: []` even for
   this program) exposes the statement-by-statement body a real projector
   needs — which local lets feed which operation call, in what order,
   with what field accesses. Building the actual `nscript → Recipe`
   projector therefore means `evermore-agentd` depending on `nscript`'s own
   library crates (`nscript-syntax`, `nscript-semantics`) directly and
   walking the checked `Program`/`CheckedProgram` AST in Rust, the way
   `nscript-cli` itself does — not shelling out to the built binary. That is
   a real new dependency (an external, pre-1.0, single-maintainer crate
   family) and a real AST walker to write, not a follow-on detail.

Both are open decisions worth settling before committing to M12.3 at full
scope, not reasons to abandon it: the type-checking half worked on the first
try, with zero changes needed on the `nscript` side.

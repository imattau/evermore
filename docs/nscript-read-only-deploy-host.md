# `nscript` read-only deploy host (M12.2) — validated end to end

Status: validated by running real binaries, not a code contribution to either
repo. No Evermore or `nscript` source changed. See
[`PROJECT_IDEA.md`](../PROJECT_IDEA.md)'s Phase 12 addendum and
[`nscript-type-reconciliation.md`](nscript-type-reconciliation.md) (M12.1)
for why this exists.

## Headline correction to the Phase 12 text's M12.2 plan

The original M12.2 plan was to "implement `nscript`'s host trait against
`evermore-operationsd`'s existing Unix-socket read path for a narrow set of
already-granted read operations (`calendar.list`/`calendar.get` or
`packages.list_installed`)." Having now read `nscript`'s actual runtime
(`crates/nscript-runtime`, `crates/nscript-cli`), that plan rests on a
premise that does not hold:

- **No NIP module declares a read/query/list operation.** Every `.nsm`
  module under `modules/std/` that this reconciliation and the type doc
  looked at (`nip01`, `nip44`, `nip51`, `nip52`, `nip78`, `nip98`) declares
  only `publish_*`/`encrypt_text`/`decrypt_text`/`authenticate_http`
  operations — all writes or transforms, routed through `OperationHost::call`
  (`crates/nscript-runtime/src/lib.rs:1838`). There is no `OperationHost`
  method or module operation shaped like "read" at all, so there is nothing
  for Evermore's catalogue reads (`calendar.list`, `packages.list_installed`)
  to be wired *onto* on the `nscript` side. Catalogue reads and `nscript`
  module operations are two unrelated vocabularies.
- **`OperationHost` has no real implementation in the CLI at all, for any
  command.** `nscript-cli`'s `deploy_program` hardcodes
  `simulated_operations(&graph, &[])` for every module operation call and
  prints "module operation calls (kick, react, publish_report, ...) are
  still simulated here; only publish/sign/relay are real"
  (`crates/nscript-cli/src/main.rs:1246`, deploy's own doc comment at
  line ~1067 confirms the same). There is no CLI flag to substitute a real
  `OperationHost`. Building an Evermore-side adapter for it today would have
  nothing in `nscript`'s own CLI to plug into — that part of the integration
  is blocked on upstream `nscript` work, not on Evermore.
- **Reads in `nscript` happen through `RelayHost`/`SubscriptionHost`
  (`query`/`subscribe`/`poll` against NIP-01 filters), and that layer is
  already real.** `nscript deploy --relay <wss://url>` constructs a
  `RealRelayPool` (`crates/nscript-runtime/src/lib.rs:6596`) that opens an
  actual WebSocket (`tungstenite`) to the given relay, publishes real signed
  events, and for each declared handler, subscribes with a real NIP-01 filter
  and polls until it sees a real `EOSE`
  (`crates/nscript-runtime/src/lib.rs:6443`). This is not simulated and
  needs no adapter — it is a generic Nostr relay client that works against
  any NIP-01-compliant relay.

So "give `nscript` a real read-only deploy host" was already true before
this session started, for the layer `nscript` has actually made real. The
work here was proving that Evermore's own local relay is one such target —
not writing an adapter, because the adapter this plan asked for compiles
against an interface (`OperationHost`-as-catalogue-read) that does not
exist.

## What was actually run

1. **Built `evermore-relay-supervisor`** (`relay/`, `go build`) and ran it
   against a scratch data directory on its default loopback address,
   `ws://127.0.0.1:4869` — the exact binary and listen behavior described in
   `relay/main.go`: loopback-only, kind-allowlisted, no NIP-42 gate on either
   connection or event acceptance today.
2. **Built `nscript`'s CLI** (`cargo build --release -p nscript-cli` against
   a fresh clone of `imattau/nscript`'s `main`), producing a `nscript` binary
   (the crate is `nscript-cli`, the binary is named `nscript`).
3. **Published one real, independently-signed kind:1 event into the relay**,
   using a throwaway Rust program against the same `nostr` (rust-nostr)
   crate and version (`0.45.7`) Evermore itself depends on — i.e. not
   through `nscript` at all, to keep the test honest. The relay returned a
   real `["OK", <event-id>, true, ""]`.
4. **Wrote a minimal `nscript` program** using only `nip01` and a single
   read-only handler:

   ```nscript
   use nip01

   relayset evermore = configured

   permissions {
       read Note from evermore
       log
   }

   on Note {
       print(event.content)
   }
   ```

   This declares no write, no signer, and no secret key — genuinely
   read-only, matching the design note's "read-only first" instruction.
5. **Ran `nscript deploy --relay ws://127.0.0.1:4869 --cycles 1`** against
   the running relay. Output:

   ```
   connecting to 1 relay(s)...
     connected: ws://127.0.0.1:4869
   watching 1 handler(s), polling every 1s (Ctrl+C to stop)
     log info: hello from the nscript/evermore M12.2 validation run
   cycle 1: 1 dispatched, 0 failed
   ```

   The printed line is the exact content string the independent publisher
   set in step 3. `nscript` connected to Evermore's real relay over a real
   WebSocket, issued a real NIP-01 `REQ`, received the real stored event
   after `EOSE`, and dispatched it through the handler — with no Evermore
   code changes, no `nscript` code changes, and no adapter crate.

## A non-obvious finding worth recording

A bare top-level `let x = select Note from evermore limit 10; for n in x? {
print(n.content) }` — the shape the spec's own `timeline.ns` example uses —
is **not evaluated at all** by `run`, `check`, or `deploy`.
`run_startup_sequential` (`crates/nscript-cli/src/main.rs:789`) only executes
module operation calls, top-level **publications**, and scheduled timer
bodies at startup; reads only run inside an `on <Event> { ... }` handler
body (dispatched per delivered event) or, presumably, a schedule body. Both
`nscript run <that-shape>` and `nscript deploy <that-shape>` silently print
nothing — not an error, just no execution path for it. The working
read-only pattern is the handler form (`on Note { print(event.content) }`)
shown above, which is what this validation actually used.

## What this means for Phase 12 going forward

- **M12.2 is done for the relay layer, with no code to merge anywhere.**
  Evermore's local relay is already a valid `nscript deploy` target for any
  script that only reads (subscribes) or publishes plain NIP-01 events. No
  host-trait adapter is needed or buildable today.
- **The module-operation-call gap (`nscript`'s simulated `OperationHost` in
  `deploy_program`) is real but is upstream `nscript` work, not Evermore
  work.** Evermore cannot contribute a real implementation until `nscript`'s
  CLI has an extension point for one. This should be raised as an `nscript`
  issue/RFC rather than attempted inside the Evermore repo.
- **M12.3 (the recipe compiler) is unaffected by this correction** — it was
  always scoped to emit `evermore-agentd`'s existing step IR and let the
  loader re-validate it, not to depend on `nscript`'s `OperationHost`. If
  anything, this finding reinforces that choice: `nscript`'s own module
  operations are not real end-to-end yet even in its own CLI, so a
  recipe compiler that depended on them being real would inherit that gap.
- **Evermore's relay has no NIP-42 auth gate today** (confirmed by reading
  `relay/main.go` for this test). That is fine for this loopback-only,
  already-trusted-process use case, but it means nothing in this validation
  exercised `nscript`'s `RelaySessionHost`/`authenticate` path — a future
  check once/if the local relay adds NIP-42 would need to re-validate that
  specifically.

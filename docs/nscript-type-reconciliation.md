# `nscript` type reconciliation (M12.1)

Status: pure documentation. No Evermore code path changed to produce this.
See [`PROJECT_IDEA.md`](../PROJECT_IDEA.md)'s Phase 12 and the "`nscript` —
Typed Recipe Authoring and a Real Deploy Host" addendum for why this exists.

This maps [`imattau/nscript`](https://github.com/imattau/nscript)'s typed NIP
modules (as of its `main` at the time of writing, commit-pinned by the
`Cargo.lock`-style module files under `modules/std/`) onto how Evermore
actually represents the same NIPs today, module by module. The scope is the
five NIPs Phase 12 named: 01, 44, 51, 78, 98.

**Headline correction to the Phase 12 text's first draft:** Evermore does
*not* hand-roll NIP-01/44/51/78 types in `evermore-common`. Raw protocol
primitives (events, kinds, tags, NIP-44 encryption) come from the external
[`nostr`](https://crates.io/crates/nostr) (rust-nostr) crate, pinned at
`0.45.7` and pulled in per-daemon with specific features
(`evermore-signer/Cargo.toml`: `nip44, os-rng`; `evermore-operationsd`:
`nip59, os-rng`; `evermore-enroll`/`evermore-pkcs11`: `nip46, os-rng`).
`evermore-common` only defines Evermore's own catalogue/grant/audit layer on
top of those primitives — it never redefines what a NIP-44 ciphertext or a
NIP-51 list *is*. That changes the reconciliation question from "nscript's
types vs. evermore-common's types" to "nscript's types vs. rust-nostr's
primitives, and separately, nscript's per-operation `permission` tag vs.
Evermore's much richer grant/approval/redaction model" — which is the more
useful comparison anyway, since the second one is where the real design risk
sits for M12.2's deploy host and M12.3's recipe compiler.

## NIP-01 — events, tags, metadata

**nscript** (`modules/std/nip01/0.1.0.nsm`): a `MetadataContent` record
(`name`, `about`, `picture`), `Note`/`Metadata` events with explicit
`kind`/`mode` (`regular`/`replaceable`), and `Person`/`EventRef` tag records
with positional wire fields (`tag Person { wire: "p" field pubkey: PubKey at
1 ... }`).

**Evermore**: no equivalent typed record anywhere in the workspace. Every
daemon that builds or reads an event (`evermore-operationsd`,
`evermore-settingsd`, `evermore-signer`, `evermore-blossom-fs`,
`evermore-npack`) uses `nostr::Event`/`nostr::EventBuilder`/`nostr::Tag`
directly, with kind numbers as plain `u16` constants (e.g.
`evermore-npack/src/lib.rs`'s `KIND_SOFTWARE_APPLICATION: u16 = 32267`,
`KIND_RELEASE_ARTIFACT_SET: u16 = 30063`). Metadata (kind:0) is not
specially modeled in Evermore at all; the project's identity model is
pubkey-centric, not profile-centric.

**Agreement:** structurally close — both ultimately describe the same
NIP-01 shape (kind, tags, content), and rust-nostr's own `Event`/`Tag` types
already cover what `nip01/0.1.0.nsm` declares. There is nothing Evermore-
specific to reconcile here; this is squarely an "`nscript` vs. rust-nostr"
question, not an "`nscript` vs. Evermore" one.

## NIP-44 — encryption

**nscript** (`modules/std/nip44/0.1.0.nsm`): a nominal `EncryptedText` type
over `Bytes`, and two operations gated by a single named permission each —
`encrypt_text(...) -> Result<EncryptedText, EncryptionError> effect Encrypt
permission encrypt` and the matching `decrypt_text`.

**Evermore**: NIP-44 encrypt/decrypt is not an operation any script-level
caller can request by name at all. It is a private capability of
`evermore-signer`, invoked only as `keys.nip44_encrypt(&pk, &plaintext)` /
`keys.nip44_decrypt(&pk, &ciphertext)` inside `evermore-signer/src/server.rs`
(lines 137 and 153), behind that daemon's Unix socket, for one purpose:
sealing/opening the settings daemon's kind:30078 payloads and operationsd's
write requests. There is no `encrypt`/`decrypt` entry in the operation
catalogue (`evermore-common::operations::catalog()`) and no MCP tool for it —
`evermore-mcp`'s README is explicit that the adapter has "no raw signing"
tool.

**Mismatch, and it is the important one:** `nscript`'s model treats
encrypt/decrypt as a *callable operation* gated by a *capability permission*,
i.e. something a script can request given the right grant. Evermore's model
has no such callable at the script/catalogue layer; encryption is baked into
specific daemon-internal flows and never surfaced as a generic primitive a
recipe or script could invoke on arbitrary plaintext. A real M12.2 deploy
host must **not** implement `nscript`'s `encrypt_text`/`decrypt_text`
operations by exposing a generic encrypt/decrypt call against the owner's
key — that would be a strictly new capability Evermore has deliberately never
shipped, not a reconciliation of an existing one. If `nscript` needs this
operation for conformance, it stays simulated against Evermore, or is scoped
to a throwaway test identity that is not the owner's.

## NIP-51 — lists

**nscript** (`modules/std/nip51/0.1.0.nsm`): one generic `UserList {
identifier: Text, members: List<PubKey> }` record and a single
`publish_user_list` operation, permission `list`. Fully generic: any
identifier, any member set, one shape for every use of NIP-51.

**Evermore**: NIP-51 is used for exactly one concrete purpose today — the
`npack` trusted-publisher list — and it is explicitly called out as a reuse,
not a new mechanism: `evermore-npack/src/lib.rs` defines `KIND_TRUSTED_PUBLISHERS:
u16 = 30000` with the comment "NIP-51 categorized people list (already on the
relay's allowlist from Phase 2) — reused as-is for the trust list, no new
kind needed for it," plus a fixed `TRUSTED_PUBLISHERS_D_TAG: &str =
"evermore:trusted-publishers"`. There is no `UserList`-shaped Rust type;
`evermore-npack` reads the member pubkeys straight off the kind:30000 event's
tags and attaches a `publisher_pubkey: String` field onto its own
`PackageListing`/`ReleaseListing` structs rather than materializing a generic
list record.

**Agreement:** conceptually identical (an addressable list of pubkeys) but
structurally narrower on Evermore's side — one fixed `d` tag, one fixed
purpose, no generic "any identifier" API. `nscript`'s generic
`publish_user_list` is a reasonable superset; a deploy-host adapter for reads
(M12.2) can map `UserList{identifier: "evermore:trusted-publishers", ...}`
onto Evermore's existing trust list directly. A *write* host (out of scope
for M12.2) would need Evermore to decide whether `nscript` may author
arbitrary-identifier NIP-51 lists or only the one Evermore already has a
UI/approval story for — almost certainly the latter.

## NIP-78 — arbitrary app data

**nscript** (`modules/std/nip78/0.1.0.nsm`): one generic `AppData {
identifier: Text, content: Text }` record and `publish_app_data`, permission
`app_data`. Flat: one string identifier, one opaque string payload, no
sub-structure.

**Evermore**: kind:30078 backs the entire settings system
(`evermore-settingsd`), and it is a two-level, typed, grant-aware model, not
a flat identifier/content pair. `evermore-common::settings::Request` exposes
`ListDomains`, `Get { domain, key }`, `GetDomain { domain }`, and `Set {
domain, key, value: serde_json::Value }` — i.e. one kind:30078 event *per
domain*, each holding many keys, each value arbitrary JSON rather than text
(`evermore-settingsd/src/settings_store.rs`'s `KIND_SETTINGS: u16 = 30078`).
Every write additionally goes through `evermore-operationsd`'s grant and
owner-approval path before it is NIP-44-sealed and published; `nscript`'s
`AppData` record and its single `permission app_data` tag have no equivalent
of "per-exact-operation grant plus owner-signed approval plus an audit
journal entry," which is exactly `evermore-common::operations::Definition`'s
`required_grants: Vec<String>` + `approval: Approval` + `result_redaction:
Redaction` fields (lines 135–145) that `nscript`'s coarser model doesn't
capture at all.

**Mismatch:** `nscript`'s `AppData` is a strict subset of what Evermore's
kind:30078 usage needs (no domain/key structure, text-only content, and a
single undifferentiated `permission app_data` where Evermore has per-exact-
operation grants). A deploy-host read adapter (M12.2) can expose
`settings.get_domain`/`settings.get` fine, since those are already-granted
catalogue reads; it should **not** try to represent Evermore's settings
surface as `nscript`'s generic `AppData` record, because doing so would throw
away the domain/key structure and the grant/approval metadata that is the
whole point of routing settings writes through `evermore-operationsd` in the
first place. This is the single clearest example in this reconciliation of
the design note's claim that "Evermore's types are stricter."

## NIP-98 — HTTP auth

**nscript** (`modules/std/nip98/0.1.0.nsm`): `HttpAuthRequest { url, method
}` → `authenticate_http(...) -> Result<AuthenticatedRequest, HttpAuthError>`,
permission `http_auth` — i.e. sign a one-off NIP-98 event authenticating a
specific HTTP request with the caller's Nostr key.

**Evermore**: `evermore-mcp --http`'s loopback endpoint does **not**
implement NIP-98 at all. Auth is a static bearer secret read from
`EVERMORE_MCP_HTTP_TOKEN` (`evermore-mcp/src/main.rs`, checked to be at least
32 characters), plus Host/Origin allowlisting and a server-issued
`MCP-Session-Id`. The README is explicit that "the bearer token is a local
access secret for the configured agent identity; it does not add operation
grants" — i.e. Evermore deliberately keeps HTTP transport auth and Nostr-key
authority as two separate things.

**Mismatch, no overlap:** there is nothing to reconcile here because Evermore
has no NIP-98 code path to compare against. If a future Evermore HTTP surface
ever wants signature-based auth instead of a shared secret, `nscript`'s
`nip98` module is a plausible typed reference to build against — but that
would be new Evermore work, not a reconciliation of existing behavior.

## Summary table

| NIP | `nscript` shape | Evermore's actual shape | Verdict |
| :-- | :-- | :-- | :-- |
| 01 | Typed `Note`/`Metadata` events, positional tag records | Raw `nostr::Event`/`Tag`, no custom types | Agree via rust-nostr; nothing Evermore-specific to reconcile |
| 44 | Generic `encrypt_text`/`decrypt_text` ops, one permission each | Not an operation; private to `evermore-signer`, no catalogue entry | **Do not expose as a generic deploy-host operation** — new capability, not reuse |
| 51 | Generic `UserList`, any identifier | One fixed-`d`-tag trusted-publisher list, reused "as-is" per Evermore's own comment | Agree conceptually; Evermore's use is a narrow instance of nscript's general case |
| 78 | Flat `AppData{identifier, content: Text}`, one permission | Two-level domain/key store, JSON values, per-operation grant + owner approval + redaction | **Evermore stricter** — `AppData` cannot represent the grant/approval metadata |
| 98 | Signed-request auth op | Static bearer token, explicitly not key-based | No overlap; not applicable today |

## What this means for M12.2 and M12.3

- The only NIPs here where a **read-only** deploy-host adapter (M12.2) is a
  clean fit without inventing new Evermore capability are **01** (trivially,
  via rust-nostr) and **51** (map `nscript`'s `UserList` read onto the
  existing trusted-publisher list). **78** can expose `settings.get`/
  `settings.get_domain` as catalogue reads, but the adapter must not claim
  those reads satisfy `nscript`'s generic `AppData` contract — it should
  present them as Evermore-specific operations with their own schema, not as
  `nip78::AppData`.
- **44** should be left simulated against Evermore for now. Wiring
  `encrypt_text`/`decrypt_text` to a real host would hand `nscript` scripts a
  generic encryption oracle against a real identity — exactly the kind of
  capability the design note's "What it must never do" section rules out
  ("give `nscript` a live signer handle, a grant it did not already hold, or
  any write path that does not go through the existing signed-
  request/approval chain").
- **98** is out of scope until/unless Evermore builds a signature-based HTTP
  auth path of its own; there is nothing in Evermore today for a host
  adapter to target.
- For M12.3 (the recipe compiler), this reconciliation is the reason the
  compiler emits Evermore's existing step kinds and lets
  `evermore-agentd`'s loader re-validate everything, rather than trying to
  give `nscript` scripts a 1:1 typed view of Evermore's catalogue: the two
  type systems agree closely on raw protocol shape (NIP-01, NIP-51) and
  disagree sharply on authority shape (NIP-44, NIP-78), and authority shape is
  exactly what the loader — not the compiler — must keep as the single
  enforcement point.

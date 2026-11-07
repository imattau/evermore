# Evermore: A Nostr-Native Linux Desktop

This is a much more pragmatic and achievable vision. The "radical event-surface" model imagined replacing the desktop; this one imagines **making the familiar desktop quietly Nostr-native underneath**. The window manager, application model, and user expectations stay intact—what changes is that the system services that handle *identity, settings, notifications, files, and software* all speak Nostr as their native protocol.

Here is a replanned roadmap built around that integration-first paradigm, using the concrete pieces that already exist in the ecosystem.

## The Revised Architecture: System Services Own Nostr

The core architectural principle is that **integration lives in headless daemons and system services, not in individual applications**. Each conventional Linux subsystem gets a Nostr-aware counterpart that speaks both the standard Linux interface (PAM, D-Bus, FUSE, GVFS) and the Nostr protocol. Applications remain ordinary Linux applications; they simply gain access to richer system services.

| System Concern | Conventional Mechanism | Nostr-Integrated Service (the thing you build) |
| :--- | :--- | :--- |
| **Session Identity** | PAM + local user | A **Nostr PAM module** + a **signing daemon** that holds the key in the kernel keyring and exposes a Unix socket for signing requests. |
| **Settings Sync** | GSettings / config files | A **settings daemon** that reads and writes NIP-78 (`kind:30078`) encrypted events, syncing preferences across machines tied to the same key. |
| **Notifications** | D-Bus `org.freedesktop.Notifications` | A **notification bridge daemon** that subscribes to Nostr mention/reply/DM streams and emits native desktop notifications (and optionally forwards local notifications *into* Nostr). |
| **Files** | Local filesystem + network mounts | A **Blossom FUSE mount** or **GVFS backend** (e.g., Garland-style) that makes a Blossom server appear as a network drive authenticated by the Nostr key. |
| **Packages** | Central repo + `pacman` | **`npack`** itself: signed `.npk` archives distributed via relays and Blossom, with user-defined trust policies and *no central repository*. |

## The Local Control-Plane Relay

In addition to the per-subsystem daemons above, Evermore adopts a pattern
proven in the sibling project [`imattau/nostrhost`](https://github.com/imattau/nostrhost)
(a Nostr-native YunoHost derivative): **a local Nostr relay as the internal
control-plane bus**, not just as a login/sync transport.

Instead of each daemon inventing its own D-Bus interface or ad-hoc socket
protocol to talk to the others, the settings daemon, notification bridge,
and `npack` trust/catalogue layer become **clients of one localhost-only
relay**. They publish signed events and subscribe to the kinds they care
about; a thin projector turns the event stream into whatever local read
model each daemon needs. This replaces a growing pile of bespoke IPC
surfaces with one bus, one auth model, and one audit trail.

**What rides the relay vs. what stays off it** (mirroring NostrHost's split):

| On the local relay | Stays local, off the event stream |
| :--- | :--- |
| Settings (NIP-78), trust/allow lists (NIP-51) | The signing daemon's Unix socket — the one privilege boundary that must never be reachable over the relay protocol |
| Notification events, package/catalogue metadata | Live session state, anything ephemeral enough that durability would be waste |
| Admin/trust changes (NIP-86 + NIP-42) | — |

**Backend: khatru** (Go, MIT), the same choice NostrHost made after
evaluating alternatives (see their `RELAY-SELECTION.md`) — chosen because it
natively implements NIP-86 (relay management: allow/ban pubkeys and kinds)
with NIP-98 auth built into the dispatch, plus NIP-77 negentropy and NIP-42.
strfry is the fallback if a packaged, self-contained relay is needed for a
distro-level install.

**Governing principle — primitive-first:** before allocating an
Evermore-specific event kind, map the requirement onto an existing NIP.
Concretely:

| Need | Primitive |
| :--- | :--- |
| Desktop settings sync | NIP-78 (`kind:30078`), exactly as in Phase 2 below |
| Trusted publishers / relays / Blossom servers | NIP-51 lists |
| Relay auth (who's asking) | NIP-42 |
| Relay administration (allow/ban) | NIP-86, NIP-98-authenticated |
| Notifications | NIP-17 / NIP-59 gift-wrapped DMs, same streams the notification bridge already subscribes to |
| Package/release metadata | NIP-89 handlers, kind 32267 (software-application), kind 30063 (release-artifact sets) |

Custom kinds are allocated only when a requirement has no standard primitive
(following NIP-33's regular/replaceable/ephemeral/addressable ranges) —
and only after that mapping exercise, not before.

This also means: "NIP-86 administers the relay; Nostr events administer
Evermore." The relay's own access control (who may read/write which kinds)
is NIP-86, so there's no proprietary relay-management API layered on top —
one less thing to design, build, and secure.

## Phased Roadmap: Integration-First

The phases are ordered by dependency and risk. Each phase delivers a working, testable slice of the integrated system before moving on. Every phase's validation step is meant to run inside the build VM (`vm/`) first, with milestone numbers (`M<phase>.<n>`) tracking individual deliverables so progress is checkable without re-deriving scope each time.

**Repo layout** these milestones assume (a Cargo workspace, added as the phases land, not all at once):

```
evermore/
  Cargo.toml                    <- workspace manifest
  crates/
    evermore-common/            <- shared NIP-01/44/78/51 types, the Unix-socket RPC schema
    evermore-signer/            <- Phase 1: signing daemon
    evermore-relay-supervisor/  <- Phase 2: wraps/configures khatru, applies local policy
    evermore-settingsd/         <- Phase 2: NIP-78 settings sync
    evermore-notifyd/           <- Phase 3: notification bridge
    evermore-blossom-fs/        <- Phase 4: Blossom FUSE mount / sync daemon
    evermore-npack/             <- Phase 5: package manager core
    evermore-inferd/            <- Phase 7.5: llama.cpp supervisor and model store
    evermore-mcp/               <- Phase 7: MCP server exposing daemons to AI agents
  apps/
    evermore-panel/             <- egui control panel and desktop launcher/drawers
  tools/
    model-build/                <- Phase 7.5: GGUF/LoRA conversion and npack recipes
  packaging/
    systemd/                    <- user unit files, one per daemon
    archiso/                    <- Phase 5 ISO profile
```

### Phase 0: Toolchain & Scaffold

**Goal:** An empty-but-correct Cargo workspace that builds and runs inside the build VM, before any Nostr logic exists.

**Milestones:**
- **M0.1** — `Cargo.toml` workspace + the six crate/app stubs above, each `cargo new --lib`/`--bin` with a one-line `main`/`lib.rs`. `cargo build --workspace` succeeds on the host.
- **M0.2** — Pin the Rust toolchain: `rust-toolchain.toml` (stable, pinned version) so `vm/create-vm.sh`'s `rustup default stable` and the workspace agree exactly.
- **M0.3** — Add `nostr-sdk` (or `nostr` + `nostr-relay-pool`) as the shared Nostr types/crypto dependency in `evermore-common`, so every later daemon builds event signing/parsing on one vetted library instead of hand-rolling NIP-01.
- **M0.4** — `packaging/systemd/` directory with a template unit file (`Type=simple`, `Restart=on-failure`, runs under `systemctl --user`) that later daemons copy, so unit-file conventions are decided once.

**Validation:** `vm/connect.sh "cd evermore && cargo build --workspace"` succeeds against a git-cloned copy of the repo inside the VM (confirms the VM's `rustup`/`base-devel` toolchain actually matches what the workspace needs, not just the host's).

### Phase 1: The Signing Foundation (Identity as a Service)

**Goal:** A working Nostr signing daemon on a standard Arch install, exposing a stable interface that any application (including egui apps) can use without ever touching the raw `nsec`.

**Milestones:**
- **M1.1** — `evermore-signer` stores one `nsec` in the Linux kernel keyring (`keyctl` via the `linux-keyutils` crate), keyed by a chosen identity name.
- **M1.2** — Unix socket at `$XDG_RUNTIME_DIR/evermore/signer.sock` (`0600`, owner-only) speaking a small JSON-RPC-ish protocol defined once in `evermore-common`: `sign_event`, `get_public_key`, `nip44_encrypt`, `nip44_decrypt`, `wrap_dm` (NIP-17/59). This pattern is already production-proven in the NanoClaw project.
- **M1.3** — Multi-identity support: the daemon holds N keys (personal, work), selected per-request by an identity name parameter — not a single global key.
- **M1.4** — `packaging/systemd/evermore-signer.service` (from the Phase 0 template) starting the daemon with the session.
- **M1.5** — `apps/evermore-panel`'s first tab: unlock the daemon, list loaded identities, show each public key (npub). First integration UI, no settings/relay logic yet.
- **M1.6** — A throwaway `evermore-signer-test-client` binary (deleted or folded into integration tests once Phase 1 is done) that connects to the socket, signs a `kind:1` note, and uploads a file to a public Blossom server using that signature — proving the `nsec` never leaves the daemon process.

**Validation:** Run M1.6's test client inside the VM via `vm/connect.sh`; inspect `/proc/<signer-pid>/status` / the client's own memory to confirm the secret key bytes exist only in the daemon's process, never the client's.

### Phase 2: Settings That Follow the Key

**Goal:** Desktop preferences that sync across machines via NIP-78, so "sign in once and your settings follow you" becomes real — with the local control-plane relay established as the pattern every later daemon reuses.

**Milestones:**
- **M2.1** — `evermore-relay-supervisor` vendors/builds `khatru` (Go) as a subprocess, binds it to `127.0.0.1` only, and applies the NIP-86 policy (allowed kinds, loopback-only write access) described in "The Local Control-Plane Relay" above.
- **M2.2** — `packaging/systemd/evermore-relay.service` starts the relay before any daemon that depends on it (`After=`/`Wants=` ordering).
- **M2.3** — `evermore-settingsd` connects to `evermore-signer`'s socket (for signing) and to the local relay (for storage/sync), defines the settings schema as a single `kind:30078` event per domain (`d` tags: `evermore:theme`, `evermore:keyboard`, `evermore:panels`, `evermore:relays`), NIP-44-encrypted to self.
- **M2.4** — Bridging rule: `evermore-settingsd` also connects to the user's configured *external* relays (from the `evermore:relays` settings domain itself — bootstrapped from a hardcoded default list on first run) and selectively republishes settings events there, gated by NIP-42 `AUTH`.
- **M2.5** — Local read/write interface for other components: a second, simpler Unix socket (`settingsd.sock`) rather than full D-Bus to start — `get(domain, key)` / `set(domain, key, value)` — with a D-Bus wrapper only added later if a non-Evermore desktop component needs it (see "Key Design Decisions").
- **M2.6** — `apps/evermore-panel`'s Settings tab: edit theme/keyboard/relay list, see last-sync timestamp per domain.

**Validation:** Run two VM instances (a second overlay disk from `vm/create-vm.sh`, or a second VM profile) signed in with the same test identity; change a setting in one, confirm it appears in the other within the external-relay round trip.

### Phase 3: Nostr-Aware Notifications

**Goal:** Mentions, replies, reactions, zaps, and DMs arrive as native desktop notifications, indistinguishable in form from local application alerts — and local system events ride the same pipe.

**Milestones:**
- **M3.1** — `evermore-notifyd` subscribes to the user's external relays for the standard kinds (mentions/replies: `kind:1` with `p`-tags; DMs: NIP-17 gift-wrapped `kind:1059`).
- **M3.2** — Same daemon also subscribes to the **local control-plane relay** for system-local notification-worthy events (a convention: any event tagged `["t","evermore-notify"]` on the local relay, published by any daemon — e.g. `evermore-npack` announcing an available update).
- **M3.3** — On any matching event, call `org.freedesktop.Notifications.Notify` over D-Bus (via `zbus`) to raise a native toast — this is the one place D-Bus is used directly, since it's the existing desktop-wide notification standard, not something to reinvent.
- **M3.4** — Reverse bridge (optional, lower priority): a small D-Bus monitor that republishes local notifications as local-relay events tagged `["t","evermore-notify"]`, so they also appear in the notification center — the `spaces-os` pattern.
- **M3.5** — `apps/evermore-panel`'s Notification Center tab: merged, filterable timeline (source: Nostr social / local system), backed by a read-only subscription to both relay sources.

**Validation:** From a second identity/VM, send a DM to the test identity; separately, have `evermore-npack` (even a stub) publish one `evermore-notify`-tagged event locally. Confirm both produce a toast and both appear in the notification center, correctly attributed to their source.

### Phase 4: Blossom as a Filesystem Surface

**Goal:** Make Blossom storage a first-class part of the file manager without replacing the local filesystem.

**Implementation status (2026-11-07):** M4.1–M4.4 are implemented. The Arch
VM smoke check built the daemon, mounted the FUSE view, synced files in both
directions between the `~/Nostr` watcher and a separate mount client, and
confirmed deletion propagation. This used two clients on one VM; the
two-VM acceptance run below remains the final cross-device check.

**Milestones:**
- **M4.1** — `evermore-blossom-fs` implements the Blossom HTTP API client (upload/get/list/mirror) authorized via `evermore-signer`'s socket (NIP-98-style signed auth events for upload/list and BUD-11 scoped authorization for BUD-04 mirroring; no stored API key).
- **M4.2** — First surface: a **sync daemon** for `~/Nostr/` (simpler than FUSE to start) using `notify`/inotify — new/changed files upload to Blossom and get indexed in a `kind:30301` addressable file-index event per directory, signed by the user's key. Initial relay state is applied before local uploads; updates are ordered, and deletions propagate.
- **M4.3** — FUSE mount (`fuser` crate) as the second, richer surface — mounts a Blossom-backed directory read/write at a configurable path, with a local cache and the same signed file index. `fuse3` provides the unprivileged mount helper.
- **M4.4** — File-manager integration: a Nautilus right-click script that uploads the selected file via M4.1 and copies a NIP-92 `imeta` reference to the clipboard.

**Validation:** Drop a file into `~/Nostr/` on one VM; from a second VM/identity pairing to the same key, confirm the file is retrievable (via the FUSE mount or a direct Blossom fetch) without manually copying anything.

### Phase 5: `npack` as the System Package Manager

**Goal:** A bootable Arch-based ISO where `npack` is the primary software distribution mechanism, bootstrapped with a minimal default trust set.

**Implementation status (2026-11-08):** M5.1, M5.2, M5.4, and M5.5 are
implemented as a stateless CLI (`evermore-npack`) plus a panel tab, not a
fourth daemon -- package discovery and install don't need a live
subscription or shared cached state the way settings/notifications do, so
each command just queries the relay fresh. M5.3 (a real default trust
seed) is deliberately **not** implemented yet, same reasoning as deferring
Phase 2's default external-relay list: there is no real "Evermore
distribution publisher" key to seed with until packages are actually being
shipped, and inventing a placeholder one would just be something to rip
out later. `trust`/`untrust` are real and tested; the mechanism is ready
for a real seed whenever there's something legitimate to seed it with.

**Milestones:**
- **M5.1** — `evermore-npack` core: `.npk` archive format (a gzipped tar with `manifest.json` at the root plus payload files, built with the `tar`/`flate2` crates rather than inventing a format), publisher trust represented as a **NIP-51 list** on the local control-plane relay (`evermore:trusted-publishers`, reusing the kind already on the relay's allowlist from Phase 2), not a bespoke trust-graph file.
- **M5.2** — Package discovery via `kind:32267` (software-application) and `kind:30063` (release-artifact set) events, fetched from the publishers in the user's trust list only — no central index to query. Both are addressable (NIP-33) keyed by the package ID, so a publisher's latest release naturally replaces the previous pointer.
- **M5.3** *(deferred — see status note above)* — Default trust seed.
- **M5.4** — Install/update/remove flow: resolve the latest trusted release → fetch the archive via the Blossom client (Phase 4's pattern, reimplemented minimally here rather than shared, same per-crate-client convention the other daemons use) → **verify the downloaded bytes' SHA-256 against what the signed release event declared** (this is the actual trust check — refuses to install on any mismatch) → unpack → symlink the manifest's declared `bin` into `~/.local/bin/`. "Installed" state is just the filesystem under `$XDG_DATA_HOME/evermore/npack/<id>/` (a `current` symlink plus per-version directories) — no separate database to go stale.
- **M5.5** — `apps/evermore-panel`'s Packages tab: identity/Blossom-URL fields, a trusted-publishers list with trust/untrust, available packages (from trusted publishers only) with Install, and installed packages with Remove. Shells out to `evermore-npack --json` rather than talking to a socket, since there's no daemon to talk to.
- **M5.6** — `packaging/archiso/` profile based on `archiso`'s `baseline`: kernel, a Wayland compositor (plain Sway — see "Key Design Decisions" on not building the shell in egui), the pinned Rust toolchain, and all Phase 0-5 daemons pre-installed and enabled. Not started — a genuinely separate, large piece of work from the package-manager mechanics above.

**Validation:** Build the ISO (`mkarchiso`), boot it in a *fresh* VM (not the dev build VM — a clean target, built the same way `vm/create-vm.sh` builds the dev one), and install one new egui application end-to-end through `npack` using only the default trust set and the ISO's preconfigured relays (blocked on M5.3/M5.6).

Verified for real (not just "it compiles") on both host and the Arch VM:
published a test package, confirmed it stays invisible to `available`
until the publisher is trusted, confirmed it appears once trusted,
installed it, actually ran the installed binary, confirmed `installed`
and `remove` correctly round-trip through the filesystem-as-database (this
caught a real bug -- `archive::extract` was parsing `manifest.json` into
memory to return it but never writing it to disk, so every command after
`install` in a fresh process couldn't find it; fixed before this was ever
true). Also checked end-to-end through the panel's Packages tab, not just
the CLI.

### Phase 5.5: Identity-Native Login

**Goal:** Make signing in to an Evermore machine mean unlocking a Nostr
identity, not just unlocking a Unix account that happens to run Evermore's
daemons. Phase 6's M6.1 ("run it as a real daily session") is not
meaningful until this exists — today there is no greeter, no enabled
login session, and no connection between "which Unix user logged in" and
"which `nsec` the signer should hold." This phase is **planning-stage**:
the identity model, unlock mechanism, profile-data approach, and
device-root identity below are now decided, and the core assumption
behind the identity model (that `homed`'s record format can actually
carry a pubkey as identity metadata) has been verified for real against
a running VM, not just assumed — but no production code exists yet, the
NIP-46 unlock path needs new PKCS#11-shim work that doesn't exist, and
this phase's relationship to Phase 7's owner/service identities remains
genuinely unresolved. Treat this whole phase as not yet a committed
implementation plan the way Phases 0-5 are.

**Decided:**

- **Unix identity model: the pubkey *is* the identity.** Not a fixed
  Unix account that a greeter merely unlocks — a Unix UID and home
  directory are provisioned on first login *for that pubkey*, the way
  enterprise SSO dynamically provisions accounts. This is the option
  that actually delivers "your identity follows you between machines";
  a shallow design (one fixed account per device, greeter just decrypts
  an `nsec` into the session keyring) was considered and rejected for
  not doing that. `systemd-homed` is the intended foundation rather than
  a bespoke PAM+NSS module from scratch — it already does pluggable
  identity records, portable encrypted home directories, and existing
  greeter/PAM/NSS integration (gdm/sddm already drive it); the real work
  is a homed storage/record backend keyed by npub, not reinventing
  account provisioning. This matches the project's original architecture
  table, which already named a Nostr PAM module as the intended
  session-identity mechanism.

  **Verified for real on the VM (2026-11-07), not just read about:**
  `systemd 262` with `homed` already running and `+FIDO2 +P11KIT +TPM2`
  compiled in. Fed `homectl create -I record.json` a fully custom JSON
  identity with an arbitrary top-level `"evermore": {"npub": ..., "pubkeyHex":
  ...}` block alongside the standard fields. It round-tripped exactly —
  `homectl inspect` returned it verbatim, it's covered by homed's own
  local Ed25519 record signature (`"signedLocally":true`), and it's
  present both in `/var/lib/systemd/home/*.identity` outside the home
  and in `~/.identity` *inside* the mounted home once activated. Created
  the account, unlocked it, activated it, confirmed the home directory
  actually mounted with real skeleton files — the full loop works, not
  just `create` succeeding. So: **yes, homed's JSON record is a real,
  working place to carry a pubkey as identity metadata; no "pluggable
  backend API" had to be invented — the record format is already
  arbitrary-extensible.**

  What this *doesn't* give us for free: homed's own authentication is
  fixed to exactly four built-in factors — password, PKCS#11, FIDO2,
  recovery key — there is no way to teach homed itself to verify a
  Nostr signature or a NIP-46 approval directly. Priority 2 (FIDO2) maps
  directly onto `--fido2-device`/`--fido2-with-user-presence` etc. with
  zero new crypto work (not hardware-tested here — no FIDO2 device was
  attached to the VM — but the flags are real, documented, and the
  binary has FIDO2 compiled in). The fallback passphrase maps directly
  onto homed's native password support, also free. **Priority 1
  (NIP-46) has no native slot** — the concrete path that fits homed's
  existing extension points is writing a small PKCS#11-compliant
  provider library that proxies `--pkcs11-token-uri=` requests to a
  NIP-46 round-trip with the paired phone, rather than talking to a
  real hardware token; that provider does not exist yet and is new
  work, not assumed-easy glue.

  One operational finding worth recording so it doesn't get rediscovered
  the hard way: `homectl` the CLI drives secrets through systemd's
  interactive `ask-password` agent protocol even when a password hash is
  supplied in the identity JSON (it still needs the *plaintext* to derive
  the key that encrypts the record's `secret` section) — `sudo`'s default
  environment stripping silently swallowed a `NEWPASSWORD` env var during
  this prototype, and non-interactive callers must answer pending
  requests via `systemd-reply-password <id> <socket> <<< secret` or avoid
  the CLI entirely. Evermore's own first-run enrollment code should talk
  to homed's actual IPC (`io.systemd.Home` over varlink) directly rather
  than shelling out to `homectl`, to avoid this exact class of
  interactive-prompt friction in an automated flow.
- **`kind:0` profile data is offline-first.** First boot must not assume
  network connectivity — a greeter can't block on reaching some outbox
  relay just to show a name. So `kind:0` is authored and signed locally
  (the same way any other Evermore identity event is), stored on the
  local control-plane relay, and usable by the greeter/panel
  immediately, with no dependency on ever reaching the wider Nostr
  network. Reconciling that local event with a "real" `kind:0` the user
  may have already published elsewhere — and giving the local relay any
  ability to reach external relays at all, to pull or push it — is
  explicitly out of scope for this phase. Phase 2 deliberately keeps the
  control-plane relay loopback-only; limited, deliberate external
  connectivity for the local relay (to mirror `kind:0` or anything else)
  is envisaged as its own later phase, not a Phase 5.5 dependency or
  blocker. OS-level preferences (theme, locale, window layout) should
  stay as Evermore's own NIP-78 app-data events via `evermore-settingsd`
  (already-established pattern), not get folded into `kind:0` — desktop
  theme has no business leaking into someone's public Nostr profile.
- **Unlock priority: NIP-46 remote signer first, hardware key second,
  passphrase only as a last-resort fallback.**
  - *Priority 1 — NIP-46 ("Nostr Connect") remote signer.* Approve login
    from a phone/companion signer app, the same way NIP-46 already works
    for Nostr clients generally. This is the most Nostr-native option
    available, needs no extra hardware purchase, and — importantly —
    doesn't actually break offline-first: the approval round-trip can
    travel over the same local, loopback control-plane relay (or a
    local-network/Bluetooth-discovered session with the phone on the
    same LAN), so "offline" here means "no wide-area internet required,"
    not "no other device involved." This is a genuine architectural
    question for Phase 1's signer, not just a UI choice: in this mode
    `evermore-signer` plausibly never holds the raw `nsec` on the device
    at all for day-to-day use — it becomes a NIP-46 *client* forwarding
    sign requests to whichever device holds the key, which is a
    materially different role from "the thing that holds the key in the
    kernel keyring."

    **Resolved and implemented (2026-11-08):** `evermore-signer` now has
    a per-identity `Backing` (`backing.rs`): `Local` (today's behavior,
    unchanged — the default for every identity that predates this
    concept) or `Remote { relay, remote_signer_pubkey }`. The backing
    record is plain JSON on disk, not in the kernel keyring — a `Remote`
    record holds no secret at all (that's the entire point) and needs to
    survive reboots, which the session keyring does not. `pair NAME
    --relay URL --remote-pubkey HEX` registers an identity as
    remote-backed; `keygen`/`import` refuse a name that's already
    remote-paired, so a local secret can never accidentally get created
    for an identity meant to never have one on this device.
    `load_keys` — the single choke point every `GetPublicKey`/
    `SignEvent`/`Nip44Encrypt`/`Nip44Decrypt` request already went
    through — checks the backing first and returns a clear, honest
    `"remote signing is not implemented yet"` error for a `Remote`
    identity, rather than silently falling through to "unknown
    identity" or attempting to read a secret that was never stored.
    This is deliberately *not* the NIP-46 protocol itself (connecting to
    a relay, constructing/encrypting the request, matching the
    response) — that's still real, separate, unbuilt work. What's
    resolved here is the fork point both roles hang off: every existing
    identity keeps working completely unchanged, and remote-backed
    identities now fail loudly and specifically instead of silently or
    confusingly. Live-tested on host and VM: pairing, the `keygen`/
    `import` guard, re-pairing (relay/pubkey rotation), and an actual
    signing request against a paired identity returning the expected
    error over the real socket.

    **The homed-side NIP-46 PKCS#11 shim (`evermore-pkcs11`, started
    2026-11-08):** a real, working PKCS#11 v2.40 provider module exists
    now, not a sketch — a `cdylib` loaded by `systemd-homed` via
    `p11-kit`, exactly the way a hardware token would be. Its struct
    layouts and function table were transcribed directly from
    `/usr/include/p11-kit-1/p11-kit/pkcs11.h` on the VM (never from
    memory — this ABI has zero tolerance for a wrong field order), and
    `p11-kit list-modules` loads it and reports a correct token with no
    errors. Reverse-engineered the *exact* record schema `homed` uses
    for PKCS#11 enrollment by reading its own debug output, not
    guessing: `pkcs11TokenUri` (pointing at the **public** key object —
    homed never touches the private key at enrollment time, only at
    unlock), `privileged.pkcs11EncryptedKey[].{uri,data,hashedPassword,
    padding}`, and `secret.tokenPin`. Confirmed the real PKCS#11 call
    sequence end to end: enrollment reads `CKA_MODULUS`/
    `CKA_PUBLIC_EXPONENT` off the public key and calls
    `C_GenerateRandom` to seed the kernel RNG; unlock logs in with the
    PIN, searches for the matching private key by `CKA_CLASS`/`CKA_ID`,
    and calls `C_DecryptInit`/`C_Decrypt` with `CKM_RSA_PKCS_OAEP`
    (`homed` picks SHA-1 or SHA-256 per probe, so the mechanism
    parameter's hash algorithm has to be honored, not hardcoded — caught
    live, not in review). `C_Decrypt` does genuine RSA-OAEP math today
    against a real, locally-generated-and-persisted RSA-2048 keypair
    (same at-rest pattern as `device_identity.rs`: plaintext key
    material, root-owned, 0600).

    **What's proven vs. still blocked, both verified on the VM, not
    assumed:** the ABI, module discovery, mechanism negotiation, and
    enrollment-time object reads all work correctly against real
    `homed`. The final create-verification step — `homed`'s own
    immediate round-trip self-test, decrypting what it just encrypted —
    currently fails with `"Selected PKCS#11 object is not a private
    key, refusing"` at the `C_Decrypt` call, with a suspiciously short
    (1-byte) ciphertext passed in. Crucially, **this exact failure
    reproduces identically with real, unmodified SoftHSM** (a
    battle-tested reference PKCS#11 implementation) in this same VM —
    strong evidence this is an existing bug or incompatibility in this
    specific Arch/`systemd 262` `homed` build's PKCS#11 verification
    path, not a defect in this module. Not yet root-caused to a fix;
    the next step is either a different systemd version to test against
    or reading `systemd-homework`'s actual C source for this self-test.
    The NIP-46 round-trip itself (connecting to a relay, encrypting a
    signed request to the paired signer, matching the response) is
    still entirely unbuilt — `C_Decrypt` does real RSA today with no
    gate in front of it; that gate is the next concrete step once the
    homed-side blocker above is resolved, not before.
  - *Priority 2 — FIDO2/U2F hardware key via `systemd-homed-fido2`.* For
    a device/user with no paired phone, or no local-network path to a
    remote signer right now. Tap to unlock, fully offline, and the same
    physical key could plausibly double as a hardware Nostr signer
    later. Fingerprint (`fprintd`/libfprint) remains an optional
    *additional* convenience layer alongside either priority option, not
    a replacement for one — biometrics can't be rotated the way a key
    can.
  - *Fallback only — local passphrase.* A NIP-49 `ncryptsec`-encrypted
    `nsec` held on the device, passphrase-decrypted into the session
    keyring. Used only in genuine degraded conditions: first-run
    bootstrap before any phone or hardware key is paired, or recovery
    when neither priority option is reachable — not offered as a
    parallel everyday choice at the greeter. This directly reuses M6.4's
    planned backup/recovery format rather than inventing a second one.
  - A physical-presence-only auto-unlock (no factor at all) is
    explicitly **not** the default for any of this — it's effectively no
    authentication — though it could be offered as an opt-in
    low-security mode for dedicated single-purpose hardware.
  - Trade-off worth naming honestly for the FIDO2/local-custody path:
    once `nsec` lives inside the already-`homed`-encrypted home (see the
    identity-model decision above), unlocking the account *is* unlocking
    the key, with no compartmentalization between the two; acceptable
    because the signer daemon is still the only thing that ever sees the
    decrypted key at runtime, but worth remembering once Phase 7's
    scoped-agent-grant model is in the picture. The NIP-46 path sidesteps
    this trade-off differently — the key never touches the device at
    all — at the cost of needing a second, reachable device to sign in.
- **A device-root npub exists, generated on first run.** Mirrors
  NostrHost's own pattern directly (its roadmap calls this the "server
  npub" — a machine identity distinct from the human "owner npub" that
  gives approvals/co-signatures). Evermore needs the same split: "the
  pubkey is the identity" above describes a *human* logging in, but a
  device has state that belongs to no single human and exists before any
  human has logged in at all. The device-root npub is generated
  automatically at first boot, with no human interaction — the Nostr-
  native equivalent of a machine-id existing before any user account
  does. Its `nsec` lives with the device itself (the signer running in a
  system context), never inside a human's `homed`-encrypted home, and it
  never appears in the greeter's user picker — it is not a login
  identity. What it owns: device-level settings no single user should
  have to own, and — concretely — npack's default trust seed (M5.3,
  still deferred): the device root signs a starter trust list baked into
  the install image, which every human user provisioned on that device
  inherits as a floor and can extend, rather than a hardcoded
  distribution key baked into every binary.
- **Greeter framework: `greetd`.** A minimal, UI-agnostic greeter daemon
  rather than a full display manager — matches the project's own
  "don't build the shell in egui" stance, since the actual login UI is
  whichever stock frontend is installed (`agreety`, `tuigreet`,
  `gtkgreet`, …), not Evermore's own code.

  **Verified on the VM (2026-11-08), not assumed:** `greetd` is
  officially packaged in Arch's `extra` repo (`greetd` 0.10.3, plus
  `greetd-agreety`/`greetd-tuigreet`/`greetd-gtkgreet`/`greetd-regreet`
  as installable frontends) and installs cleanly with its own dedicated
  `greeter` system user. Its PAM config (`/etc/pam.d/greetd`) chains
  through `system-local-login` → `system-login` → `system-auth` — and
  `system-auth` already wires in `pam_systemd_home.so` at the auth,
  account, password, *and* session phases, as Arch's **default,
  unmodified PAM configuration** (confirmed by reading the actual file
  on the VM, not from memory). `/etc/pam.d/login` uses the identical
  chain. This means: **a completely stock `greetd` + stock frontend
  needs zero Evermore-specific code for password- or FIDO2-backed homed
  accounts** — `pam_systemd_home` already does the homed-aware
  authentication as part of a standard Arch install, exactly the same
  way it would for any other PAM client (`login`, `sshd`, `su`). The
  account just needs the right auth factor attached via its homed
  record (which `evermore-enroll`'s `create` already can set).

  For the NIP-46 priority-1 path, this is genuinely good news, not a
  gap: the PKCS#11 provider already identified as the concrete path
  (see the unlock-priority decision above) slots in at the **homed**
  layer, the same place FIDO2 already does — so NIP-46 support doesn't
  need any greeter-specific or PAM-specific code either, once that
  provider exists. The only Evermore-specific *greeter* work that's
  left is UI polish, not auth plumbing: showing `kind:0` profile data
  (name/avatar) at the login screen, which no stock frontend knows how
  to do. A completely stock `tuigreet` works, generically, right now
  for the password/FIDO2 paths — profile display is a later layer on
  top of a already-functional login, not a blocker for one.

**Still open:**

- **Relationship to Phase 7's owner/service-identity split.**
  `evermore-operationsd` already distinguishes an owner identity
  (approves) from a service identity (executes/attests) in VM testing
  (`--owner-identity vm-owner --service-identity vm-service`). Does
  "owner identity" just mean "the first human user npub provisioned on
  this device," and should the service identity be derived from or
  signed under the device-root npub rather than being its own
  free-floating generated key? Not resolved — needs deciding once both
  this phase and Phase 7 are further along, not guessed at now.
- **Scope boundary with Phase 6.** M6.4 (`ncryptsec` export/recovery) and
  M6.3 (true cross-device validation) both become easy to validate for
  real once this phase exists, and arguably can't be *meaningfully*
  validated without it (M6.3 in particular is currently untestable: there
  is no login flow for "the same identity" to follow across two
  machines). This phase should land before Phase 6 milestones are
  attempted, not alongside them.

### Phase 6: Dogfood & Harden

**Goal:** Stop treating this as a demo and start running it as a daily system on at least one real machine, finding the rough edges Phases 0-5's narrow validations didn't surface.

**Milestones:**
- **M6.1** — Run the ISO (or the equivalent package set on an existing Arch install) as a real daily session for a defined period; log every manual workaround needed as a tracked issue.
- **M6.2** — Security pass on the two trust boundaries that matter most: the signing daemon's socket permissions/identity-switching, and the local relay's NIP-86 policy (can anything on the loopback plane write kinds it shouldn't?). Written up the same way `imattau/nostrhost`'s `SECURITY-REVIEW.md` does it.
- **M6.3** — Cross-device validation for real: two *different physical machines* (not two VMs on one host) signed in with the same key, confirming settings/notifications/files/packages all actually follow the identity as designed, not just within one host's loopback relay.
- **M6.4** — Backup/recovery story for the `nsec`: NIP-49 `ncryptsec` export/import flow through the signing daemon, tested by deliberately wiping a VM's keyring and recovering from the exported backup.
- **M6.5** — Decide and document the public release shape (ISO download, AUR packages for the daemons on stock Arch, or both) based on what Phase 5/6 actually proved out.

### Phase 7: AI & Agentic AI Integration

**Goal:** Make AI a system capability that any Evermore component can invoke,
not a separate application with its own authority. Headless services own
inference, context, search, and operation execution. Sway, the egui panel,
`npack`, notifications, and MCP clients use the same operation catalogue,
approval rules, and audit trail. This builds on Phase 1's signer and Phase 2's
existing loopback control relay at `127.0.0.1:4869`; it does not introduce a
second relay or grant any service the owner's raw `nsec`.

**Implementation status (2026-11-08):** The initial M7.1 catalogue is
implemented in `evermore-common::operations` with versioned schemas, access
classes, grants, approval metadata, and redaction policy for identity,
notification, settings, context, local-search, and Sway capabilities. It
canonicalizes validated arguments and records the catalogue version with each
request. `evermore-operationsd catalog` serves
these definitions over a private Unix socket and prints the same data as JSON;
the panel's AI tab reads the same catalogue and shows the required grant and
approval classification. The CLI routes catalogued reads to the signer,
settings, notification, context, and semantic-index services, validates
arguments from the catalogue, and redacts secret-shaped results. MCP derives
its context/search tools from this catalogue and requires matching agent read
grants. AI-facing notification reads exclude DMs until a separate explicit
consent scope exists. Initial M7.2 support accepts
signed kind:1 requests and owner approvals, encrypts request content to the
owner, persists lifecycle state, publishes acknowledged events on the
existing local relay, and signs outcome events with a distinct service
identity. It executes approval-gated `settings.set`, Sway focus, workspace,
and application-launch operations. Owners can approve, deny, or cancel a pending request;
cancellation is persisted and returned through the signed result lifecycle.
The panel's AI tab lists pending operation requests,
shows their actor and exact arguments, and submits signed owner approvals,
denials, or cancellations through the signer. The panel also displays recent lifecycle status,
operation IDs, relay acknowledgement, approval/result event IDs, and results.
Approval is
bound to the signed request event ID and exact request payload. A persisted
`running` state becomes `execution_unknown` after restart or an ambiguous
daemon response to prevent blind replay. Before an approved side effect, the
service persists and relay-publishes a signed execution-attempt event binding
the operation ID, catalogue version, request event, and approval event; it
executes only after relay acknowledgement. The panel history exposes its event
ID and acknowledgement state. When the executor socket is unavailable, the
CLI, MCP `op_status`, and panel history/pending views fall back to the read-only
private local journal; offline panel snapshots cannot approve or deny
requests. An interrupted execution claim is conservatively reported as
`execution_unknown` without replay. M7.5 adds owner-signed, expiring
agent grants scoped to exact write operation names. Grant publication must be
acknowledged by the local relay before use; revocation is immediate in local
state and blocks queued requests, with signed relay publication retried in the
background. The grant is checked at submission, approval, and execution.
The VM validation used distinct test identities: a relay-acknowledged grant
limited an agent to `sway.switch_workspace`, the agent's self-approval was
rejected, and the owner's approval completed the request.
The initial M7.6 stdio adapter now exposes catalogue-backed MCP tools,
uses a configured dedicated agent identity, checks exact operation grants,
and provides `mcp_status` and actor-scoped `op_status`. It now also supports
Streamable HTTP on a loopback-only listener with bearer authentication,
Host/Origin validation, secure session IDs, and required negotiated protocol
headers. HTTP calls still route to the same configured agent identity and
operation-grant checks. The endpoint returns JSON responses and rejects GET
with 405 because it does not provide server-initiated SSE streams. M7.3 now has
an initial `evermore-inferd` service with a versioned Unix socket API, streamed
chat, installed-model listing/unloading, and per-request CPU/GPU/auto backend
selection through a loopback Ollama-compatible backend. Npack manifests can
declare a GGUF file and digest; installation retains the signed release event
and verified archive, and `evermore-inferd` verifies the release signature,
package/version binding, archive hash, and GGUF hash before importing into
local Ollama. This is exposed through the approval-required
`ai.model.install_signed_npack` operation and performs no download. The panel's
AI tab lists installed local models, persists the selected model and backend in
a private per-user preference file, and streams chat through `evermore-inferd`.
The loopback Ollama-compatible backend is a first implementation and is
superseded by Phase 7.5, which replaces it with a supervised `llama-server`
while keeping this socket contract. If no model is installed, it explains the
signed package install/import path.
No cloud provider is exposed by this service. VM runtime validation imported locally
available BGE and TinyLlama GGUFs from pinned, publisher-signed NPK releases;
each package install and model import used distinct-owner approval and
relay-acknowledged lifecycle events. BGE returned 384-dimension embeddings and
powered explicit-file indexing/search with secret redaction. The reusable VM
check runs both embeddings and streamed TinyLlama chat in a private Bubblewrap
network namespace; both succeeded while an external TCP connect failed with
`ENETUNREACH`. The VM panel selected the installed TinyLlama package, stored
the model/backend preference with mode 0600, and its selected model completed a
streamed inference request.

**Service boundaries:**

| Service | Responsibility | Authority |
| :--- | :--- | :--- |
| `ai-inferd` | Runs locally installed models and streams inference responses. The initial service has no cloud-provider path. | Thinks; cannot execute system operations. |
| `ai-contextd` | Provides selected session context such as focused window, workspace, theme settings, and non-DM notification metadata. | Observes only; serves only sources enabled by the user-session allowlist. |
| `ai-indexd` | Builds and searches a local semantic index for explicitly selected text files. | Searches only; indexes only per-file selections and redacts secret-shaped values. |
| `evermore-operationsd` | Owns the operation catalogue, authorization checks, approval state, execution, result redaction, and audit events. | The only path for AI-requested mutations; executes writes only after required approval. |
| `ai-agentd` (future optional) | Could coordinate multi-step tasks using inference, context, and registered operations. External agents use MCP in the initial implementation. | No shell or execution authority; not shipped in this phase. |
| `ai-policyd` (future optional) | Could supply advisory risk and trust summaries to the human approval flow. | No grant or approval authority; not shipped in this phase. |
| `evermore-mcp` | Adapts registered operations to MCP tools and transports. | Interface only; contains no policy or execution authority. |

All AI-facing services bind to Unix sockets or loopback. They never listen on
public interfaces by default. Model inputs and operation results pass through
scope checks and secret redaction before reaching a model. The human approval
surface may display fuller details than the model receives.

**Lessons applied from NostrHost:** NostrHost's own README describes a
pre-alpha whose remaining work is smoothing the full install → use → recover
journey. Its control plane also keeps the human owner identity distinct from a
server-generated identity, and treats administrative actions as signed,
auditable actions. Apply those lessons by proving one complete, recoverable
Evermore path before expanding the number of AI services: start with one
catalogue-backed read and one approval-gated write, exercise denial, timeout,
restart, and recovery, then add inference and broader integrations. Reuse
Evermore's existing relay at `127.0.0.1:4869`; do not copy NostrHost's relay
address or introduce another bus. Keep the human/session actor, agent actor,
and service signer distinct, and derive the actor from authenticated transport
or session credentials rather than a caller-supplied field.

The companion `nostrhost-mcp` design makes a further useful boundary concrete:
the adapter translates protocol and schemas, while the installed system owns
authorization, approval, execution, and operation state. In Evermore, route
all interfaces through `evermore-operationsd`; bind every approval to an
immutable operation request (actor, operation name, canonical arguments, and
request ID), and revalidate that binding and current grant when execution is
claimed. Persist lifecycle state so `operation_id` status queries and recovery
after a daemon restart do not depend on an MCP connection staying open. Keep a
local adapter health/identity check available even when the relay or executor
is unavailable, and make secret redaction a shared result boundary before
content reaches any model. These are implementation requirements, not extra
authority for the model or MCP client.

**Milestones:**

- **M7.1 — One operation catalogue.** Define each operation once with its
  name, JSON input/output schema, read or write classification, required
  grant, approval requirement, and result-redaction rules. Generate the CLI,
  egui controls, and MCP tool schemas from this catalogue so those surfaces
  cannot drift into different permissions or behavior. Add canonical argument
  normalization and a stable operation version so an approval is bound to the
  exact request that the user reviewed.
- **M7.2 — Signed operation control plane.** Add `evermore-operationsd` as
  the executor for local operations such as settings writes, package actions,
  and Sway IPC. Requests carry a Nostr actor identity and are correlated to
  approval, execution, and result events on the existing local relay. Keep the
  current signed kind:1 event format with operation marker tags: [NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md) classifies
  kinds 2200–2205 as ephemeral, so they would not meet the durable recovery
  requirement. The local relay already accepts kind:1; no custom NIP-86
  allowlist is needed. Store the lifecycle as regular, immutable events. The agent signs its request,
  the owner signs approval, and the executor signs
  execution/result events; event IDs and `operation_id` correlate the chain.
  Local reads may execute immediately.
  Writes return an `approval_required` state with an `operation_id` until
  approved through the egui panel, an owner-controlled NIP-46 bunker, or an
  administrator CLI. Persist explicit `pending`, `approved`, `denied`,
  `running`, `succeeded`, `failed`, `expired`, and `cancelled` states; use
  idempotent request handling and a single execution claim so retries,
  duplicate events, or daemon restarts cannot replay a write. Approval itself
  only changes eligibility; the executor reloads and rechecks the operation,
  grant, and approval before making a side effect. Expose `operation_id` status
  through CLI, egui, and MCP. The panel approval inbox and recent-activity view
  now provide the egui status and decision surface for the initial operations.
- **M7.3 — Local inference service.** Initial `evermore-inferd` is implemented
  with a stable Unix socket API, streamed responses, installed-model
  listing/unloading, and per-request CPU/GPU/auto selection over a loopback
  Ollama-compatible backend. It rejects cloud model tags and exposes no
  remote-provider path. Signed `npack` model import is implemented through an
  approval-required operation: the importer checks the retained signed release,
  package/version binding, archive hash, and manifest GGUF hash before local
  Ollama import. Offline VM validation uses signed locally imported BGE and
  TinyLlama packages with networking disabled in a private network namespace.
  The panel provides first-run local model selection, backend preference, and
  streamed chat, with signed-package installation guidance when no model is
  installed. The local CPU chat and embedding paths have been smoke-checked on
  the host; VM validation
  imported the signed models through owner approval and returned embeddings
  plus streamed chat. Any future cloud path
  requires per-request consent and clear disclosure of the context leaving the
  device.
- **M7.4 — Context and semantic search.** Initial read-only `ai-contextd`
  and `ai-indexd` services are implemented. `ai-contextd` serves only
  allowlisted compositor, theme-settings, and non-DM notification sources;
  `ai-indexd` indexes explicitly selected individual text files. It
  uses the local embedding service, stores a private local vector index, and
  redacts secret-shaped values before embedding and storage. Selection is
  configured through `EVERMORE_INDEX_FILES`; only regular `.md`, `.txt`,
  `.rst`, and `.org` files up to 2 MiB are accepted. The AI tab now manages
  per-file selections, reindexing, removal/purge, and semantic search through
  a private config file, with an explicit picker for supported files in
  `~/Nostr`. Context sources include installed package manifests. The AI tab
  controls the allowlist for compositor, theme, non-DM notification metadata,
  and installed packages through a private config file. VM context validation
  returned all installed package manifests. The current disposable test set
  contains signed BGE and TinyLlama model packages plus a dependency-report
  fixture.
  Host socket smoke checks pass for indexing/search
  and default-deny context source access; both services compile on the VM. The
  shared operation catalogue exposes context listing/reading and semantic
  search to CLI/MCP; MCP checks the matching agent grant for these calls.
  Private messages and other sensitive sources remain excluded unless a
  future explicit consent flow scopes them. Context subscriptions must not
  become an alternate write path. The VM compositor-context smoke check
  returned its live focused workspace. VM runtime validation installed a
  local embedding backend and successfully indexed and searched an explicitly
  selected fixture file. A separate VM context query returned the installed
  package manifest from the complete current NPK package set.
- **M7.5 — Scoped agent and approval enforcement.** Agent identities are
  separate Nostr keypairs, never the owner's key. Owner-signed grants constrain
  exact operation names, expire within 365 days, and require relay
  acknowledgement before use. `evermore-operationsd` checks the grant at
  request submission, approval, and execution; only the owner can approve, so
  an agent cannot approve its own request. Revocation takes effect locally
  immediately, prevents queued work from executing, and publishes a signed
  revocation with background retry. The exact-operation grant milestone is
  complete for settings, notification, package, model-import, Blossom, and
  Sway writes. Future hardening may add addressable grants, additional scope
  levels, signer-side grant enforcement, and more write executors.
- **M7.6 — Thin MCP adapter.** The initial stdio adapter is implemented as
  `evermore-mcp`. It derives tools and schemas from the shared catalogue,
  binds the process to a configured dedicated agent identity in the owning
  user session, checks exact operation grants, and submits writes only through
  the signed approval path. It exposes `mcp_status` and actor-scoped
  `op_status`, with no raw signing or shell passthrough. Actor-scoped
  `op_status` remains available from the local journal while the executor is
  stopped. Streamable HTTP is
  implemented with a loopback-only bind, required bearer token, Host and
  Origin validation, per-session IDs, protocol-version checks, body/header
  limits, and JSON responses. GET is rejected with 405 because this adapter
  does not initiate SSE streams. The token grants access to the configured
  dedicated agent identity; operation grants remain enforced independently by
  the adapter and operations daemon. MCP remains an interface, not an
  authority boundary.
- **M7.7 — Sway and egui integration.** Initial `evermore-sway-ai-bridge`
  reports focused-window/workspace changes to `ai-contextd`. The catalogue
  exposes window listing plus approval-gated focus, numbered workspace, and
  application-launch operations; launch accepts a bounded argv and invokes the
  executable directly without a shell. `evermore-operationsd` rechecks a focus target before acting and
  records ambiguous Sway responses as `execution_unknown`. The panel has an
  owner approval/denial inbox that signs decisions through the signer. It now
  has a catalogue-driven generic write proposal form: select a loaded signer
  identity and approved operation, enter JSON arguments, and the panel
  validates, encrypts to the configured owner, signs, and submits through the
  operations daemon. A compact catalogue-search palette now opens from the
  global Sway binding `$mod+Shift+space` through
  `evermore-panel --command-palette`; it uses the same grant and approval path
  and has no direct command execution. `evermore-panel --ai` opens the full
  local-model view directly. VM validation opened two temporary
  windows and confirmed an owner-approved `sway.focus_window` request focused
  the specified container ID after the other window had focus. An
owner-approved `sway.launch_app` request started the harmless `true`
executable and emitted acknowledged request, approval, execution, and result
events. Host and VM `cargo test --offline --workspace` pass, as do host
`cargo check --offline --workspace`, `cargo fmt --all --check`, and
`git diff --check`. Local
  speech-to-text voice input may be added later
  through the same inference and operation path.
- **M7.8 — Existing services use the same operations.** The catalogue now
  routes settings writes through approval, filters DM records from AI
  notification reads, supports approval-gated notification dismissal and clear, and
  accepts approval-gated package install and removal proposals. Installs bind
  to an exact package ID, version, and SHA-256 from a trusted signed release;
  removals target one validated package ID and remove its package-managed
  executable link. Catalogue reads now expose installed package manifests and
  latest versions plus archive hashes from the owner's trusted-publisher
  list, supporting trust-graph-grounded discovery and exact-version update
  explanations. The operation daemon invokes `evermore-npack` with the
  owner's configured identity for trusted-release lookups and installs; set
  `EVERMORE_BLOSSOM_URL` in its service environment. `npack` rechecks the
  trusted signed release and archive hash before extraction. This path compiles
  on host and VM. A trusted publisher release was installed from a pinned
  signed listing through `packages.install_pinned` in the VM; a distinct owner
  approved it, and request, approval, execution, and result events were
  acknowledged. A disposable local notification was also dismissed through
  `notifications.dismiss` after owner approval, with the other fixture
  notification left in place. `blossom.list_owned` uses owner-signed NIP-98
  authentication; `blossom.delete` verifies ownership in that listing and
  requires owner approval. A VM disposable-blob flow exercised listing,
  short-lived exact-operation grant, approved deletion, and relay acknowledgement
  of request, approval, execution, and result; the grant was revoked afterward
  and the empty listing confirmed. `packages.explain_dependencies` reports
  direct dependencies and installed versions; VM validation distinguished the
  installed BGE and TinyLlama packages from a missing fixture dependency.
  Exact-version installation/update is supported; richer batch/update planning
  and additional notification triage remain future work. An agent may draft a
  package update but
  publishing still requires the publisher identity's own signature.
  No `ai-policyd` is shipped in this implementation; notification and package
  trust decisions remain with the owner rather than being inferred by policy.
- **M7.9 — Activity log and optional agent collaboration.** The panel's recent
  activity view renders operation IDs, actor, operation, request/approval/
  execution/result publication status, approval/execution/result event IDs,
  arguments, and outcome from the durable local operation store. The immutable
  result event records status only; details remain in the permissioned local
  journal. Manual encrypted export is implemented: it is disabled unless
  `EVERMORE_ACTIVITY_EXPORT_RELAY` names a user-selected relay and the owner
  invokes `evermore-operationsd export`. The command encrypts the full local
  operation/grant snapshot to the owner with NIP-44, publishes service-signed
  parts with acknowledgements, and tags each part with an export ID and index.
  Optional scoped sub-agents may later use
  their own Nostr keys or owner-mediated NIP-46 signing, with no shared owner
  secret and no self-approval.

**Verification progress (2026-11-08):** Full workspace `cargo check --offline`
and `cargo test --offline --workspace` pass on host and VM; the signer
socket-roundtrip integration test passed in both environments. Host runtime checks exercised local CPU chat, embeddings,
file indexing/search, scope removal/purge, context source denial, and package
manifest context. VM runtime checks exercised live Sway context, bridge updates,
and an approval-gated workspace switch; a separate VM flow verified the exact
agent grant was relay-acknowledged, agent self-approval was rejected, owner
approval succeeded, and request/approval/result event IDs were queryable from
the local activity history. After adding execution-attempt events, a separate
isolated VM store exercised an owner-signed Sway workspace request end to end:
the distinct service identity's execution event was relay-acknowledged before
the workspace command, and request, approval, execution, and result events all
showed relay acknowledgement with final status `succeeded`. A host runtime
check also queried the agent's operation through both the CLI and MCP after
stopping the executor; both returned the actor-scoped record from the local
journal. A host runtime
smoke check exercised the MCP HTTP
initialize/session and tool-list flow, including bearer-token and Origin
rejection. A VM runtime flow used temporary owner, agent, and service
identities plus an isolated operation journal: the relay acknowledged
exact-operation grants; a granted `settings.list_domains` call succeeded over
Streamable HTTP; `settings.get` outside the grant was refused; `settings.set`
returned `pending`; the owner denied it; and actor-scoped `op_status` returned
the denied record. Request, approval, and result events were acknowledged, and
the denied write did not execute. A second VM flow exercised
`packages.remove` through MCP against a disposable package fixture: it stayed
pending until owner approval, then removed only the fixture and finished with
request, approval, execution, and result acknowledgements. Replaying that exact
signed request after an operations-daemon restart returned the stored
`succeeded` outcome without repeating the removal. A separate pending settings
request remained `pending` after daemon restart and was then owner-denied with
acknowledged approval and result events. The package catalogue reads were also
called through VM MCP: `packages.list_installed` returned a disposable fixture
manifest, and `packages.list_available` returned a valid empty list for an
owner with no trusted publishers. The optional activity export was invoked
against the local relay with a disposable journal; a mismatched owner identity
was refused, then the configured owner target produced a NIP-44-encrypted,
service-signed event and received the relay acknowledgement. A VM model flow
used the local BGE GGUF artifact without pulling a model, installed its
publisher-signed package at a pinned version and archive digest through the
`packages.install_pinned` operation, and passed model import through a distinct
owner's approval. Both package installs and model imports succeeded with
relay-acknowledged lifecycle events. Embedding and explicitly selected
text-file index/search succeeded; the returned excerpt redacted the fixture's
password-shaped value. TinyLlama also streamed chat tokens from the inference
socket while running in a private network namespace; an external TCP
connection was unreachable. One deliberately space-constrained install left
an execution claim interrupted; daemon restart reported `execution_unknown`
without replay. After removing duplicate test copies, a new owner-approved
install completed successfully. The VM panel displayed and selected
`evermore/p7.model.tinyllama:1.0.0`, stored the selection with private file
permissions, and a request through the inference socket streamed the expected
short response. A disposable
notification dismissal succeeded through owner approval and removed only the
selected fixture; a focus request also succeeded for the requested Sway
container ID after starting two temporary windows. In the isolated VM journal, a
modified copy of a signed request was rejected with an invalid event ID; after
simulating an expired deadline and restarting the daemon, the request became
`expired` and owner approval was refused. The new
palette was built and launched in VM Sway; the configured rule produced a
centered 720×560 floating window, and the VM's original Sway config was restored
after the visual check. Blossom listing and deletion were exercised against a
disposable VM blob: a narrowly scoped grant enabled the agent request, the owner
approved deletion, all four lifecycle events were acknowledged, the grant was
revoked, and a follow-up listing was empty. Dependency explanation returned
the two installed model dependencies and one absent fixture dependency. A
separate pending owner request was cancelled before execution: it finished as
`cancelled`, its request, owner decision, and result were relay-acknowledged,
and it had no execution event.

**Validation status:** The initial Phase 7 acceptance checks are complete on
the VM: CLI/MCP catalogue reads, owner-gated writes, denial and cancellation,
pending approval across restart, exactly-once recovery behavior, expiry,
duplicate and modified request handling, grant scope and self-approval
rejection, local status fallback, signed lifecycle acknowledgements, and
secret redaction were exercised. Read and write surfaces use the same
catalogue; MCP exposes no shell or direct Sway tool, and the panel routes
writes through owner approval. The reusable
`vm/phase7-offline-inference.sh` check verifies embeddings and streamed chat
while external network access is disabled. Host and VM workspace tests pass.
A host application that separately gives an external agent shell access must
enforce that boundary at its own process sandbox. Remaining M7 enhancements
are explicitly optional or future work in the milestone notes (for example,
voice input, batch update planning, server-initiated MCP SSE, broader grant
scopes, and optional agent/policy daemons); they are not prerequisites for the
initial Phase 7 baseline.

### Phase 7.5: Inference Runtime Refactor — `llama.cpp` Supervisor

**Goal:** Replace the loopback Ollama backend that M7.3 selected with a
supervised `llama-server` child process, so Evermore owns the inference path
end to end — model storage, signed artifact pairing, quantization, and
lifecycle — while the `ai-inferd` Unix-socket contract that the panel, drawer,
`ai-indexd`, and MCP already depend on stays stable. This is
integration/refactor work: no new authority, no new daemon, and no cloud path.

**Implementation status (2026-11-08):** M7.10–M7.17 are implemented, with two
artifact-dependent checks outstanding. `evermore-common` defines the versioned
`ModelManifest` (an adapter requires the operation-registry version it was
trained against), store discovery via `EVERMORE_MODEL_DIRS` with user-root
precedence, and protocol version 2. `evermore-inferd` scans
`/var/lib/evermore/models` and `~/.local/share/evermore/models`, spawns one
`llama-server` child per model on an ephemeral loopback port with a per-process
API key, restarts after a crash, and serves `/v1/chat/completions` SSE and
`/v1/embeddings` behind the unchanged socket contract. `UnloadModel` stops that
model's child and `Health` reports `llama.cpp`. Signed npack import keeps the
retained release, archive, and base/adapter hash verification, then publishes a
manifest plus GGUFs into the model store instead of uploading to Ollama, and
`evermore-indexd` discovers a model that declares the `embeddings` capability
instead of defaulting to an Ollama tag.

M7.14 adds `ListAdapters`/`SetAdapterScale` over the socket, backed by
`llama-server`'s `/lora-adapters` API; scale is runtime state and a restart
returns to the manifest's scale. M7.15 accepts an optional GBNF grammar on chat
requests (64 KiB cap, no NUL bytes), and the offline check forces an exact
operation-registry JSON object. This `llama-server` build (Arch `llama-cpp`
0.6.0) rejects `json_schema`/`response_format` with a sampler error, so callers
convert a schema to GBNF rather than sending one directly. M7.16 adds
`tools/model-build/` recipes for base conversion (`Q4_K_M` default), adapter
conversion (`F16`), and `.npk` assembly with hash verification. M7.17's offline
check now also covers grammar-constrained chat, adapter-scale refusal on a
model without an adapter, and skipping an invalid store manifest.

VM validation with external networking disabled scanned two models, returned a
384-dimension embedding, streamed normal and grammar-constrained chat, blocked
external egress, restarted a killed child, unloaded it, and verified signed
import into the store. Arch's package is `llama-cpp` (it provides
`llama-server`), so packaging and the VM script target that name. Two checks
remain artifact-dependent: end-to-end adapter hot-swap needs a trained LoRA
GGUF in the test set (flag construction, manifest pairing, and scale validation
are covered), and the ISO default pair is not pre-installed until the Phase 5
ISO profile exists. Existing disposable model fixtures must be republished with
`capabilities` (or `EVERMORE_EMBED_MODEL` set) before `ai-indexd` can select
them.

**Why not Ollama:** Ollama's value proposition is model management for end
users; Evermore is a distribution that must own the inference path, pair base
models with signed adapters, and update both through `npack` with provenance.
Ollama abstracts exactly the decisions Evermore needs control over: adapter
support, quantization choice, storage layout, the conversion pipeline, and the
update cadence. The removal of `ADAPTER` support is a concrete reminder that
Ollama's roadmap is not Evermore's. `llama.cpp` is the reference implementation
for LoRA adapters and GBNF grammars, Arch tracks it closely, and the
distribution can depend on the packaged `llama.cpp` (plus its `-cuda`,
`-vulkan`, or `-hip` variants) rather than building C++ inside a Rust build.

**Architecture — supervisor, not embedder:**

```
panel / launcher / left drawer / ai-indexd / ai-agentd
        │  Unix socket (stable ai-inferd protocol)
        ▼
    ai-inferd ──spawns──► llama-server (127.0.0.1:<ephemeral>)
        │                   --model base.gguf --lora evermore.gguf
        ├─ model-store scan + manifest validation
        ├─ adapter pairing, process pool, request queue
        └─ crash detection, restart/backoff, redaction boundary
```

The daemon spawns and supervises `llama-server`; it does not link `llama.cpp`.
That keeps packaging on the Arch package, isolates model or driver faults from
the daemon that every launcher query depends on, and gives Evermore
`llama-server`'s OpenAI-compatible `/v1/chat/completions` streaming endpoint,
`/v1/embeddings`, GBNF/JSON-schema constrained output, and the runtime LoRA API.
Embedding via `llama-cpp-rs`, or switching to the pure-Rust `mistral.rs`,
remains a back-pocket option only: tighter integration at the cost of build
fragility is not a good trade for a distribution that ships to unknown
hardware.

**What Evermore now owns:**

| Concern | Ollama's behaviour | What Evermore must build |
| :--- | :--- | :--- |
| **Model storage** | `~/.ollama/models`, content-addressed blobs | `/var/lib/evermore/models` for system models and `~/.local/share/evermore/models` for user models, merged by an `ai-inferd` scan |
| **Model discovery** | `ollama pull` from a registry | Signed `npack` packages that place a GGUF plus a manifest in the model store |
| **API surface** | `/api/chat`, `/api/generate` | `llama-server`'s OpenAI-compatible endpoints behind `ai-inferd`'s Unix socket |
| **Versioning** | Tags and digests | `npack` package versions and signed manifests; the base + adapter pairing is a versioned artifact |
| **Quantization** | Curated by Ollama | Chosen per artifact: `Q4_K_M` base, `F16` adapter, `Q8_0` for quality-over-size |
| **Conversion** | Built in | `convert_hf_to_gguf.py` and `convert_lora_to_gguf.py` at build time |

**Design decisions:**

- **Subprocess supervisor.** Spawn `llama-server` on an ephemeral loopback
  port; `ai-inferd` owns lifecycle, restart/backoff, and request multiplexing.
  Keep `--ctx-size` conservative (8k–16k is ample for launcher and drawer use;
  KV cache is the memory constraint on small devices).
- **Process per model.** Each loaded base model gets its own `llama-server`
  child, because chat and embedding models need different flags and CPU/GPU
  selection becomes launch flags (`--n-gpu-layers`). A bounded pool with idle
  eviction replaces Ollama's `/api/ps` unload; `UnloadModel` stops the
  corresponding child. Model switching is a restart, not an in-process swap.
- **Store roots.** System artifacts live under `/var/lib/evermore/models/` and
  user artifacts under `~/.local/share/evermore/models/`. `ai-inferd` scans
  both and merges them, with the user root taking precedence on name conflict.
- **Manifest.** Each `npack` model package carries a versioned manifest: base
  GGUF filename and hash, optional adapter GGUF filename and hash, context
  size, quant, and the operation-registry version the adapter was trained
  against. `ai-inferd` reads it on startup and refuses to pair an adapter with
  a mismatched base or registry version.
- **Signed artifacts only.** Every loadable model comes from a signed `npack`
  release whose signature and hashes are retained, so provenance is checkable.
  There is no unsigned local-GGUF path, and Ollama's content-addressed blobs
  are not adopted because they cannot be tied back to a signed manifest; the
  migration story is reinstall from `npack`.
- **Consumers use the socket.** The panel, drawer, `ai-indexd`, and MCP speak
  `ai-inferd`'s Unix socket, never `llama-server`'s loopback HTTP. If the
  runtime is ever swapped, one daemon changes, not every consumer.
- **Default pair.** The ISO pre-installs a `Q4_K_M` 1B–3B base plus the `F16`
  Evermore adapter, both signed `npack` packages.
- **Grammar-constrained output.** GBNF or JSON-schema constraints on chat
  requests let `evermore-operationsd` and agent tooling require valid
  operation-registry JSON. This shapes model output; it grants no authority.

**Milestones:**

- **M7.10 — Model store and manifest.** Add the versioned manifest schema to
  `evermore-common` and extend `evermore-npack`'s `AiModelPayload` from
  `{gguf_file, sha256}` to base/adapter artifact names and hashes, context
  size, quant, and trained operation-registry version. Implement the dual-root
  store scan with merge, validation, mismatch refusal, and unit tests.
- **M7.11 — Supervisor lifecycle.** Spawn `llama-server`, allocate an ephemeral
  loopback port, probe readiness, restart with backoff after a crash, reap
  orphans on shutdown, bound the process pool, and evict idle children. Update
  `packaging/systemd/evermore-inferd.service` (binary path, model roots,
  context size) and declare the Arch `llama.cpp` dependency.
- **M7.12 — Protocol translation.** Replace the Ollama HTTP client with
  `llama-server`'s `/health`, `/props`, `/v1/chat/completions` SSE streaming,
  and `/v1/embeddings`, mapping `Backend` to launch flags. Bump
  `evermore_common::ai::PROTOCOL_VERSION` and update the panel, `ai-indexd`,
  and MCP consumers; keep `Request`/`Event` semantics otherwise unchanged.
- **M7.13 — Signed import without Ollama.** Keep the existing
  release-signature, archive-hash, and GGUF-hash verification, but install the
  verified artifacts into the model store with their manifest instead of
  uploading blobs and calling `/api/create`. Derive model listing, digest,
  size, and quant from the store scan; update the
  `ai.model.install_signed_npack` description and make package removal delete
  the paired artifacts.
- **M7.14 — Adapter pairing and hot-swap.** Launch the base with `--lora` and
  use `llama-server`'s `/lora-adapters` API to adjust adapter scale at runtime.
  Enforce base-hash and registry-version pairing, expose adapter state through
  the socket, and leave per-query-class adapters as future work.
- **M7.15 — Grammar-constrained output.** Accept an optional GBNF grammar or
  JSON schema on chat requests, exercise it against the operation catalogue's
  JSON so proposals are always parseable, and keep it a model-output constraint
  rather than a caller-controlled bypass.
- **M7.16 — Quantization and packaging.** Define the per-artifact quant policy,
  add `npack` recipes for base and adapter GGUFs, and put
  `convert_hf_to_gguf.py` / `convert_lora_to_gguf.py` in build tooling beside
  the operation registry so the adapter version tracks the registry version.
  Pre-install the default pair in the ISO profile.
- **M7.17 — Migration and acceptance.** Provide an explicit reinstall path for
  existing installs, update `vm/phase7-offline-inference.sh` to launch
  `llama-server` instead of `ollama serve`, and validate streamed chat,
  embeddings, adapter hot-swap, crash restart, grammar-constrained output, and
  mismatch refusal in the offline network namespace. Run the full workspace
  check, test, format, and `git diff --check`.

**Validation:** In the VM, with external networking disabled, install the
signed base and adapter packages, stream chat and embeddings through the
unchanged `ai-inferd` socket, change the adapter scale at runtime, kill
`llama-server` mid-request and confirm the daemon restarts it without losing
its own socket, request catalogue JSON under a grammar, and confirm a
mismatched base/adapter manifest is refused. Confirm the panel and `ai-indexd`
need no protocol changes beyond the version bump.

**Build order:** Manifest and store scan first; supervisor and protocol
translation next; signed import after that. Adapter hot-swap and grammar
follow once baseline chat and embeddings pass. Packaging, ISO pre-install, and
the reinstall path finish the phase.

### Phase 8: Familiar Desktop Experience

**Goal:** Turn the Nostr-aware system services into a conventional, keyboard-
driven Sway desktop. Nostr remains infrastructure for identity, sync, software
provenance, and optional cross-device features; it is not the desktop's visual
theme. A user unfamiliar with Nostr should still recognize the launcher,
notification inbox, settings, workspaces, and system tray. AI is an optional
fallback: when deterministic search can answer a query, no model is invoked.

**Implementation status (2026-11-08):** M8.1 has a tested Waybar preset for
Sway workspaces 1–9, window title, network, audio, battery, clock, and tray.
The launcher spike rejected `nwg-drawer` after the VM showed that its first
printable key is consumed during its search-focus handoff. Walker and
Elephant's desktop-applications provider were also installed and tested, but
Walker lost the first typed character in Sway both as a resident service and
on-demand, even with exclusive layer-shell keyboard focus. Fuzzel 1.15.0 from
Arch's official repository is now the active candidate: its drun mode searches
installed applications without a generic command runner or separate provider
service. The updated binding and Fuzzel preset are in place; final hotkey and
launch validation is still pending. No custom launcher rendering or
layer-shell code has been added.

**Product rules:** Keep the default Waybar panel conventional: workspaces,
clock, tray, and available system status, with no npub, relay health, or trust
indicators. Put identity and relay details under account/software-source
settings. Keep the activity log in Recent Activity settings rather than making
protocol activity a top-level desktop surface. Nostr mentions and DMs join
ordinary desktop notifications; the inbox is not a Nostr feed. Reads can run
immediately within their configured scopes; system writes continue through the
Phase 7 operation catalogue, owner approval, and signed audit lifecycle.

**Reuse-first implementation plan:** Use upstream desktop components for
generic shell behavior, and add Evermore-specific code only where the product
needs its identity, context, or operation semantics.

- **Panel:** configure [Waybar](https://github.com/Alexays/Waybar) and its
  existing Sway, clock, tray, audio, network, and battery modules. Do not write
  a panel or status-module framework.
- **Launcher:** use [Fuzzel](https://codeberg.org/dnkl/fuzzel) in desktop-run
  mode for the initial keyboard-first app search. It supports native Wayland
  layer-shell, exclusive keyboard focus, fuzzy matching, icons, desktop-action
  search, and recent-launch sorting without an extra daemon. `nwg-drawer` and
  Walker were rejected after both lost the first typed character during Sway
  focus handoff in the VM. Fuzzel has no generic shell-command mode when
  invoked with `--show drun`. Keep system mutation in Evermore's catalogue and
  retain `evermore-panel --command-palette` as the safe Evermore action surface
  until it can be integrated cleanly as a launcher provider/menu.
- **Right inbox:** use
  [SwayNotificationCenter](https://github.com/ErikReider/SwayNotificationCenter)
  for D-Bus toasts, notification history, grouping, DND, and its layer-shell
  control center. Have local and Nostr sources enter through the existing
  `evermore-notifyd`/D-Bus path. Represent an approval notification as a
  pointer to the pending operation and open the signed Evermore approval UI;
  do not implement a second approval authority inside the notification
  daemon. First check whether SwayNC's notification list and configurable
  widgets can provide the desired inbox; build a custom inbox only for the
  missing source/thread/triage behavior.
- **AI session drawer:** reuse the current `evermore-panel` AI clients and
  operation flow. Start as a normal floating eframe window positioned by Sway
  rules so context, session, and approval behavior can be validated before
  taking on layer-shell focus. If a true edge-anchored drawer proves necessary,
  add one layer-shell integration to this client; do not build a general
  widget toolkit. [Eww](https://elkowar.github.io/eww/eww.html) is suitable for
  disposable layout prototypes, but its GTK/CSS layout and animation limits
  make it a poor foundation for the stateful AI session and context tray.
- **Drawers and motion:** Sway's layer-shell protocol already lets clients
  anchor and overlay surfaces. Use an existing layer-shell client for generic
  surfaces and reserve custom code for Evermore content. Begin with instant
  show/hide and correct keyboard focus. If a slide is needed after usability
  tests, animate the drawer client-side. Keep vanilla Sway as the supported
  default; [SwayFX](https://github.com/wlrfx/swayfx) adds compositor-level
  animations and layer effects but is a fork and remains an optional profile,
  not a Phase 8 dependency.

**Milestones:**

- **M8.1 — Conventional Sway and Waybar baseline.** Configure upstream
  [Waybar](https://github.com/Alexays/Waybar) rather than writing a panel.
  Define the default Sway
  keybindings, workspaces 1–9, Waybar layout, notification daemon, and user
  services. Keep Nostr-specific status out of the default panel. Preserve the
  existing VM configuration and provide a reproducible configuration/install
  path for the target Arch desktop.
- **M8.2 — Launcher v0, then v1.** Configure upstream Fuzzel in drun mode for
  Super+Space. V0 shows installed apps on empty query and supports
  first-keystroke filtering, arrow selection, Enter launch, and Escape
  dismissal. Verify hotkey focus and launch from a clean Sway session. V1 adds
  deterministic settings/file search and desktop actions through supported
  launcher modes or a provider only if the extra integration is useful. No
  chat or AI is needed. Keep system actions in the Evermore palette. Only
  build a custom launcher surface if upstream clients cannot meet the
  interaction and extension requirements without a fork; do not keep two
  competing default launcher bindings.
- **M8.3 — Right inbox drawer.** Configure SwayNC's existing control center
  first as the glanceable, dismissible triage surface. Build a custom drawer
  only if its supported widgets/actions cannot present the required data. The
  inbox should group local notifications and pending owner approvals by source
  and thread, with urgent items first. Reuse the notification data shown in
  toasts; support open, dismiss, approve, reply, and mute actions only where
  the source and operation catalogue permit them. Add Nostr mentions/replies
  and DMs as another source through `evermore-notifyd`; keep the feed itself in
  a Nostr client. Keep ordinary notifications chronological with a DND toggle;
  optional trust grouping is off by default. Clipboard events remain opt-in.
- **M8.4 — Context capture contract.** Add context objects with source,
  resolution, scope (one turn or pinned), and refresh policy (snapshot or
  refresh per turn). Resolve context through `ai-contextd`: extract, normalize,
  redact, summarize when needed, then present. The UI tray must show what the
  model actually receives, including truncation and secret-redaction notices,
  with a way for the user to inspect the sanitized payload. Refuse paths marked
  no-AI. Show token estimates and model-window budget; never silently drop
  pinned context. This does not widen `ai-contextd`'s allowlist or grant a new
  write path.
- **M8.5 — Left AI session drawer.** Add a 380–420 px, summonable session
  drawer with a 40 px collapsed rail, context tray, context-grounded suggested
  actions, named/resumable local sessions, streamed responses, and inline
  operation cards. Every mutation uses the existing Phase 7 request and
  approval lifecycle. AI suggestions appear only after deterministic launcher
  results and only for open-ended queries. Keep the drawer responsive while
  inference runs; cancel a response independently of operation cancellation.
  Build it against the Phase 7.5 runtime: the drawer talks only to
  `ai-inferd`'s socket and does not depend on the backend's HTTP shape.
- **M8.6 — Reliable context entry points and polish.** Support paste, internal
  egui drag-and-drop, a capture hotkey, and an application “Send to AI” action.
  External Wayland drag-and-drop is an enhancement, never a required path.
  Start with focused-output placement and close on output focus change. Use
  floating surfaces with zero exclusive zone by default; a pinned-drawer mode
  may reserve space. Begin with instant visibility changes; if user testing
  shows a slide animation is valuable, animate that client surface rather than
  requiring SwayFX.
- **M8.7 — Semantic and AI launcher fallback.** Add embedding-based search
  without changing the deterministic result path. Keep exact/fuzzy first
  results within a 16 ms target and semantic results within 50 ms. Only
  genuinely open-ended queries may start inference; show its visually distinct
  card below deterministic results, only if the first token arrives within
  300 ms, and allow cancellation with a 3 s response budget. If local hardware
  cannot meet the first-token target, omit the card. The launcher never opens
  or persists a chat; conversation belongs to the left drawer. This consumes
  `ai-indexd` and the Phase 7.5 inference service; the deterministic path must
  never start either.

**Validation:** In the Arch VM, verify keyboard focus and text entry on summon,
empty-query app grid, deterministic search without starting inference, launch,
settings sub-actions, Esc/focus-loss dismissal, and measured first-result
latency. Verify the inbox unifies a local toast and pending approval without
duplicating records, and that each action calls its owning service. For the AI
drawer, test a file snapshot and refreshed context, secret redaction, no-AI
path rejection, truncation/token-budget disclosures, pinned-context behavior,
stream cancellation, and an approval-gated write. Confirm every surface works
with unavailable inference and that disabling optional Nostr sources leaves a
useful conventional desktop.

**Build order:** Launcher v0 and the Waybar baseline first; right inbox with
local notifications and approvals next; then context capture and the left
session drawer; add Nostr inbox sources and semantic/AI launcher fallback only
after deterministic paths and the context-grant display are validated. Layer-
shell keyboard focus, exclusive-zone reflow, external drag-and-drop, and
multi-output placement are known risks; validation tests focus early and
defers external drag and persistent per-output drawers.

## Key Design Decisions and Risks

A few decisions will shape the entire project and are worth resolving early:

- **Trust bootstrapping for `npack`.** The system needs a usable default trust set on first boot, or the package manager is a blank slate. Decide whether the ISO ships with a curated publisher list, a "trust on first use" model, or a hybrid. The `npack` project's explicit "no central repository" design means *you* must provide the initial social graph seed.
- **Key storage strategy.** The kernel keyring is excellent for session-scoped keys and is already used by the signing daemon pattern. For at-rest backup and cross-device portability, NIP-49 `ncryptsec` is the right format. The system secret service (GNOME Keyring / KWallet) is a viable alternative for desktop-integrated storage. Pick one primary path and make the signing daemon the only component that ever sees the decrypted key.
- **`smithay-egui` is not a shell.** It is explicitly "mainly intended for debug interfaces". Do **not** plan to build the entire desktop shell in egui. Use egui for the *applications and control panels* (settings, package manager, notification center) and rely on a conventional Wayland compositor (e.g., Sway, or a Smithay-based compositor that is not trying to be a full shell) for window management. This keeps the risk bounded.
- **D-Bus vs. Unix socket for the signing interface.** A Unix socket is simpler and more portable across languages; D-Bus gives you standard service discovery and permission integration. The NanoClaw pattern uses a Unix socket with a read-only mount into containers, which is a clean security boundary. Start with a Unix socket and add a D-Bus wrapper later if integration with existing desktop services demands it.
- **NIP-78 encryption and relay auth.** NIP-78 settings should be encrypted with NIP-44 and relays should require NIP-42 `AUTH` before serving `kind:30078` events. This is not optional for a production system—without it, settings are either public or served to the wrong clients.
- **AI has no authority outside the operation executor.** Inference, context, search, policy advice, and MCP transports stay separate. Every AI-initiated mutation enters through the registered operation and approval path; a model or adapter never gets a shell or direct service bypass.
- **Evermore owns the inference runtime.** `ai-inferd` supervises `llama-server` as a child process instead of embedding `llama.cpp` or delegating to Ollama. Models are signed `npack` artifacts placed in a defined store, and every consumer uses the `ai-inferd` socket rather than the runtime's loopback HTTP. This keeps packaging on the Arch package, isolates model or driver crashes from the daemon the launcher depends on, and leaves the runtime replaceable without changing consumers.
- **Agent identities are never the owner's key.** Any AI/agentic integration (a local assistant, an MCP-connected coding agent, an autonomous process) gets its own Nostr keypair and an explicit, scoped grant — never the owner's real `nsec`, and never permission to approve its own privileged requests. This is the same split `imattau/nostrhost` already uses for its agent/owner model (see Phase 7); Evermore reuses it rather than re-deriving a weaker version from scratch.

Phase 7's initial implementation and VM acceptance are complete. The separate
Phase 4 two-device acceptance run is still recorded as outstanding; keep it as
a project-wide readiness gate where required by the dependency order. Phase 7
future enhancements are listed in its milestone notes and do not block its
initial baseline.

Phase 7.5 is a refactor of the inference runtime, not a further Phase 7
enhancement: it replaces the Ollama-compatible backend recorded above with a
supervised `llama-server` while preserving the `ai-inferd` socket contract. Its
runtime, packaging, and VM checks are implemented; end-to-end adapter hot-swap
and the ISO default pair remain artifact-dependent (see its status note). It
does not change Phase 7's acceptance evidence, but it must complete before
Phase 8's AI drawer and semantic launcher fallback are considered complete.

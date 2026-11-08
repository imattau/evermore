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
    adapter-training/           <- Phase 7.5: catalogue-derived data, training, evaluation
  packaging/
    systemd/                    <- user unit files, one per daemon
    archiso/                    <- Phase 9 ISO profile and installer
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
- **M5.3** *(deferred — see status note above; delivery path specified in Phase 9's device-root seed)* — Default trust seed.
- **M5.4** — Install/update/remove flow: resolve the latest trusted release → fetch the archive via the Blossom client (Phase 4's pattern, reimplemented minimally here rather than shared, same per-crate-client convention the other daemons use) → **verify the downloaded bytes' SHA-256 against what the signed release event declared** (this is the actual trust check — refuses to install on any mismatch) → unpack → symlink the manifest's declared `bin` into `~/.local/bin/`. "Installed" state is just the filesystem under `$XDG_DATA_HOME/evermore/npack/<id>/` (a `current` symlink plus per-version directories) — no separate database to go stale.
- **M5.5** — `apps/evermore-panel`'s Packages tab: identity/Blossom-URL fields, a trusted-publishers list with trust/untrust, available packages (from trusted publishers only) with Install, and installed packages with Remove. Shells out to `evermore-npack --json` rather than talking to a socket, since there's no daemon to talk to.
- **M5.6** — `packaging/archiso/` profile based on `archiso`'s `baseline`: kernel, a Wayland compositor (plain Sway — see "Key Design Decisions" on not building the shell in egui), the pinned Rust toolchain, and all Phase 0-5 daemons pre-installed and enabled. Not started — a genuinely separate, large piece of work from the package-manager mechanics above, and now specified in detail as **Phase 9**, which replaces this milestone's "pinned Rust toolchain" with prebuilt binaries.

**Validation:** Build the ISO (`mkarchiso`), boot it in a *fresh* VM (not the dev build VM — a clean target, built the same way `vm/create-vm.sh` builds the dev one), and install one new egui application end-to-end through `npack` using only the default trust set and the ISO's preconfigured relays (blocked on M5.3/M5.6, both absorbed by Phase 9).

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

    **Both blockers resolved for real (2026-11-08), root-caused by
    reading systemd's and p11-kit's actual source, not by guessing:**
    the `"Selected PKCS#11 object is not a private key, refusing"`
    failure was never a bug in this module. Cloning `systemd` and
    grepping for that exact string led straight to
    `src/shared/pkcs11-util.c`'s `pkcs11_token_find_private_key()`: it
    inspects the `pkcs11:` URI's *own* embedded `type=` component before
    ever calling into the module, and hard-refuses if that component
    says `public` rather than `private`. The enrollment command had
    supplied `--pkcs11-token-uri=...type=public` (correct for reading
    the public key at enrollment) — but that exact URI string gets
    persisted into the record and reused verbatim by
    `homework-pkcs11.c`'s decrypt callback later, tripping this
    hard-coded check before the module is ever asked anything. **Fix:**
    omit `type=` from the enrollment URI entirely; a bare
    `token=...;id=...;object=...` URI isn't read as asserting a class at
    all, so `find_private_key` falls through to searching generically
    and reaches the module correctly. (This explains why real SoftHSM
    hit the identical failure earlier — it's a caller-side URI-usage
    issue, not anything module-specific, confirming the earlier
    suspicion.) With that fixed, a second, this module's *own* bug
    surfaced immediately: `CKR_DATA_LEN_RANGE` was transcribed as `0x90`
    in `ffi.rs` instead of the real `0x21` — `0x90` is actually
    `CKR_OPERATION_ACTIVE`, so a genuine RSA-decrypt failure was being
    reported as "another operation is already taking place," a deeply
    misleading error that pointed nowhere near the real cause. Fixing
    that constant exposed the real problem underneath: `C_Decrypt` was
    hardcoded to OAEP-SHA1 while `homed` had negotiated OAEP-SHA256
    (confirmed via `C_DecryptInit`'s mechanism parameter, which was
    already being parsed but not honored) — OAEP decryption fails
    outright on a hash mismatch. Fixed by reading
    `CK_RSA_PKCS_OAEP_PARAMS.hash_alg` in `C_DecryptInit`, storing the
    negotiated hash per-session, and using the matching `sha1`/`sha2`
    variant in `C_Decrypt`.

    **Verified end to end on the VM after both fixes:** `homectl create`
    with the corrected URI succeeds; `homectl activate` unlocks a real
    LUKS-encrypted home through this module's genuine RSA-OAEP-SHA256
    decrypt, with the home directory actually mounting and populated
    skeleton files present — confirmed by directly listing
    `/home/<user>`, not just checking exit codes. Temporary file-based
    diagnostic logging (`dbg_log`) used to trace the exact call sequence
    during this investigation has been removed now that both root
    causes are fixed and verified; it isn't appropriate to leave
    ad-hoc, unencrypted protocol tracing inside a crypto-handling module
    long-term.

    **The NIP-46 approval gate itself (built 2026-11-08):**
    `C_Decrypt` now blocks on a live NIP-46 round-trip
    (`nip46_gate.rs`) before any RSA-OAEP math runs, closing the gap
    noted above. Two already-established identities are reused rather
    than invented: the device-root key (`evermore-signer`'s
    `device_identity.rs` state file, read directly since this module
    runs inside `systemd-homework` with no signer daemon or session
    keyring available) and a separate pairing record
    (`pkcs11-nip46-pairing.json`: `relay` + `remote_signer_pubkey`) that
    deliberately does *not* reuse `evermore-signer`'s per-identity
    `backing::Backing::Remote` records, since this gates the device's
    own unlock, not any one human identity's signing. The approval
    itself rides on NIP-46's standard `get_public_key` (to learn who
    will actually sign) followed by `sign_event` on a small,
    human-readable, nonce-bound event describing the unlock attempt —
    exactly the action a real signer app (Amber, nsec.app, ...) already
    prompts its user to confirm, rather than an invented method. A
    validly-signed response whose event id matches what was requested
    (which alone proves pubkey/content/kind/tags all matched, since
    `EventId` hashes all of them together) is the approval; a decline,
    a mismatch, or a timeout all fail closed, as does having no pairing
    configured at all — this path refuses by design until a remote
    signer is actually paired, rather than silently falling back to
    unauthenticated RSA math.

    **Verified twice, not just unit-tested:** a self-contained Rust test
    (its own throwaway, unfiltered relay plus a mock signer in a
    separate thread, real NIP-44 encryption and real secp256k1
    signing/verification, no shortcut into the module's internals)
    proves the wire protocol end to end. Separately, against real
    `systemd-homed` on the VM: with no pairing configured, both
    `homectl create` (which calls `C_Decrypt` itself, to verify the
    encrypted key round-trips before committing the account) and
    `homectl activate` now fail closed, where they used to silently
    succeed. With a pairing configured and a standalone mock-signer
    process answering over a real (throwaway) relay, both succeeded —
    `homectl activate` unlocked a real LUKS home only after the mock
    signer's log showed it receiving and approving the exact nonce-bound
    unlock request, three separate times across one create+activate
    cycle. One deliberate exclusion from this: the already-running
    local control-plane relay (`relay/main.go`, `ws://127.0.0.1:4869`)
    was *not* reused for either test, nor is it a candidate for
    production pairing — it's loopback-only and enforces a kind
    allowlist that excludes 24133 (NIP-46) on purpose, since it's for
    intra-device daemon traffic, not for talking to an external phone.
    Production's `relay` field must always point at a real, externally
    reachable relay the paired phone can also see.

    Still open: the actual pairing *flow* (how a phone and a device
    agree on a `pkcs11-nip46-pairing.json` in the first place) doesn't
    exist yet — today it's written by hand. That belongs with the
    greeter/enrollment work, not this module.
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
  provider exists. The only Evermore-specific *greeter* work left was
  UI polish, not auth plumbing: showing `kind:0` profile data
  (name/avatar) at the login screen, which no stock frontend knows how
  to do — a completely stock `tuigreet` already worked, generically,
  for the password/FIDO2 paths, so this was never a blocker for login
  working at all. **That UI polish is now built — see
  `evermore-greeter` below.**

  **Verified end to end on the VM (2026-11-08):** `evermore-enroll create`
  gained an `--identity-pubkey NPUB|HEX` flag that merges the
  `"evermore":{"npub":...,"pubkeyHex":...}` block into the record being
  created (parsed via `nostr`'s `PublicKey::parse`, which accepts either
  form) — closing the gap between "the pubkey-is-identity model is
  proven to round-trip through homed" (verified earlier, by hand, via
  `homectl -I`) and "`evermore-enroll` actually sets it" (previously
  the caller had to hand-author that block into the JSON file itself).

  Then proved the real login path, not just `CreateHome`/`ActivateHome`
  over D-Bus: provisioned an evermore-tagged account with a real
  `privileged.hashedPassword` (homed refuses `CreateHome` outright with
  "User record has no hashed passwords, refusing" if this is missing —
  `--secret-password` alone only supplies the transient unlock secret,
  not the persistent hash homed needs to accept the account at all),
  then drove a real login through real `greetd` — speaking its IPC
  protocol directly (`greetd-ipc(7)`: length-prefixed JSON over
  `$GREETD_SOCK`) rather than through `agreety`/`tuigreet`'s UI, since
  that protocol *is* what a stock frontend speaks and bypassing just the
  terminal UI, not greetd or PAM, keeps the test meaningful while
  staying scriptable. The journal shows `greetd`'s real PAM stack doing
  the work: `pam_systemd_home(greetd:auth): Home for user evgreettest
  successfully acquired`, then a real `pam_unix` session opened as that
  user with the home actually mounted (`~/.identity`,
  `~/.identity-blob/`, skeleton files all present). From *inside* that
  live session, `homectl inspect`'s `.evermore.npub` and a fresh
  `evermore-signer import` of the matching `nsec` (done in-session,
  since the session keyring only exists for the life of the PAM
  session) produced the identical pubkey — confirming the whole chain
  (enroll → homed record → real greetd+PAM login → session keyring)
  actually carries the same identity through, not just each piece in
  isolation. Cleaned up afterward: greetd's config restored, test
  account removed, no scratch files left on the VM.

- **`evermore-greeter`: a real graphical greetd frontend with kind:0
  avatars (built 2026-11-08).** A new crate, `apps/evermore-greeter` —
  an eframe/egui app run inside `cage` (a minimal kiosk Wayland
  compositor, the same pattern `gtkgreet` uses), replacing the stock
  `agreety`/`tuigreet` frontend greetd's own config points at
  (`packaging/greetd/config.toml`). It enumerates provisioned
  identities via homed's real D-Bus API (`ListHomes` +
  `GetUserRecordByName`, reading each record's `evermore.pubkeyHex`),
  shows each one's cached display name and avatar in a picker, and
  drives the actual login by speaking greetd's own IPC protocol
  directly (`greetd-ipc(7)`: length-prefixed JSON over `$GREETD_SOCK`)
  — this binary genuinely *is* a frontend, the same way `agreety` is,
  not a shortcut around one. All real authentication stays entirely
  PAM's/homed's job, untouched; the greeter only ever relays prompts
  and passes the typed answer through.

  **The profile data itself:** `evermore-signer` gained a `set-profile`
  subcommand (`set-profile NAME --display-name NAME [--avatar PATH]`)
  that writes a plain-file cache — `/var/lib/evermore-profiles/<pubkey-hex>/{profile.json,avatar.png}`
  — and best-effort publishes the equivalent signed `kind:0` event to
  the local relay. The file cache is what actually matters for the
  greeter: the local control-plane relay is a systemd *user* service
  that only starts after a login session already exists, so there is
  no relay to query before anyone has logged in, for any identity —
  `kind:0`-on-the-relay is for every other consumer (the panel, etc.),
  not this one.

  **A real design conflict found and fixed, not hypothetical:** the
  obvious location, `/var/lib/evermore/profiles`, doesn't work —
  `/var/lib/evermore` itself is already actively managed as a 0700,
  root-only secrets store by both `evermore-pkcs11` and this crate's
  own `device_identity.rs`, each of which unconditionally
  `set_permissions(dir, 0o700)`s it on every bootstrap. A
  world-readable subdirectory nested inside it would get silently
  re-locked the next time either of those happened to run. Fixed by
  making the cache a *sibling* directory, `/var/lib/evermore-profiles`
  (`packaging/systemd/evermore-profiles.conf`: `1777 root root`, like
  `/tmp` — world-writable-but-sticky, since each user only ever creates
  and writes their own `<pubkey-hex>` subdirectory, and the contents
  are already public by nature, the same data a `kind:0` event carries).

  **Verified end to end on the VM, including two real UI bugs found
  only by actually driving it** (via `wtype` for keyboard input, since
  this runs headless — the picker also supports pressing `1`-`9` to
  select an identity, a real accessibility feature, not just a test
  hook, which is what made driving it via keyboard alone possible at
  all): (1) `TextEdit::request_focus()` was being called every frame
  instead of once, which silently re-stole focus back onto the
  password field the instant Enter tried to surrender it to submit —
  broke every way to submit the form (Enter, Tab, clicking "Log in").
  Fixed by tracking focus-requested state explicitly and requesting it
  exactly once per prompt. (2) `vertical_centered` did not actually
  center a nested `horizontal` row of avatar tiles in this egui
  version — confirmed visually (the row rendered pinned to the left
  edge under a correctly-centered heading); fixed by computing the
  row's centering offset by hand from the known tile width, after also
  discovering the whole screen needed wrapping in `CentralPanel` to
  have a stable width to center against at all (`eframe::App::ui`'s own
  doc comment: "no margin or background" — easy to miss that this also
  means no defined width for a bare `Ui`).

  Also needed one more system package the project hadn't needed before:
  `vulkan-swrast` (lavapipe, a software Vulkan implementation). Without
  it, Mesa's EGL falls back to `zink` (OpenGL-over-Vulkan) on this
  headless, GPU-less VM and fails outright with
  `VK_ERROR_INCOMPATIBLE_DRIVER` — root-caused via
  `VK_LOADER_DEBUG=all`, not guessed, and documented in
  `packaging/greetd/README.md` so it isn't rediscovered the hard way
  again. On real hardware with a GPU this isn't needed at all; `cage`
  picks up the real driver automatically.

  Two full logins confirmed via real `greetd` + real `pam_systemd_home`
  (journal: `Home for user evgreeter2 successfully acquired`, home
  actually mounted) driven entirely through the picker UI: first with
  no profile set (renders the initials-fallback avatar, a colored
  circle with the name's first letter), second after running
  `set-profile` with a real generated PNG from inside a live session —
  the picker rendered that exact image, cropped circular, with the
  configured display name, before any password was typed. Cleaned up
  afterward: greetd's stock config restored, test account and all
  scratch profile data removed, no stray processes left on the VM.

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
- **M6.5** — Decide and document the public release shape (ISO download, AUR packages for the daemons on stock Arch, or both) based on what Phase 5/6 actually proved out. Phase 9 records the decision for the first release: the offline ISO is the deliverable, with packages for stock Arch as the secondary path.

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

**Implementation status (2026-11-08):** M7.10–M7.20 are implemented. A first
adapter was trained and hot-swapped locally; the signed adapter release and ISO
pre-install remain. `evermore-common` defines
the versioned `ModelManifest` (an adapter requires the operation-registry
version and training provenance it was built against), store discovery via
`EVERMORE_MODEL_DIRS` with user-root precedence, and protocol version 2.
`evermore-inferd` scans `/var/lib/evermore/models` and
`~/.local/share/evermore/models`, spawns one `llama-server` child per model on
an ephemeral loopback port with a per-process API key, restarts after a crash,
and serves `/v1/chat/completions` SSE and `/v1/embeddings` behind the unchanged
socket contract. `UnloadModel` stops that model's child and `Health` reports
`llama.cpp`. Signed npack import keeps the retained release, archive, and
base/adapter hash verification, then publishes a manifest plus GGUFs into the
model store instead of uploading to Ollama, and `evermore-indexd` discovers a
model that declares the `embeddings` capability instead of defaulting to an
Ollama tag.

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

M7.18 adds `tools/adapter-training/`, whose Rust generator reads
`evermore_common::operations` directly and emits deterministic train/eval JSONL
plus a manifest carrying the catalogue fingerprint; synthetic catalogue data is
the default, and opt-in local traces are redacted recursively and hashed. The
current catalogue produces 78 train and 49 eval records covering every
operation. M7.19 adds a versioned PEFT training config and script plus an
evaluator that reports parse rate, tool selection, schema validity, refusal
accuracy, unexpected operations, and base-vs-adapter and scale sweeps against a
local `llama-server`. A first run on TinyLlama-1.1B (124 train / 72 eval, 12
epochs, 372 steps) improved parse rate from 0.764 to 0.972, operation accuracy
from 0.423 to 0.594, and schema validity from 0.099 to 0.609 at adapter scale
1.0; refusal accuracy stayed at 0.0 and is the gate before publishing. M7.20
records training provenance in the model package
(base, dataset, config, eval-report, catalogue fingerprint, trace contribution)
and adds `registry.lock` with `check-registry.sh`; the lock currently pins the
catalogue fingerprint with no adapter published yet, and
`--require-adapter` fails until the release fields are filled.

VM validation with external networking disabled scanned two models, returned a
384-dimension embedding, streamed normal and grammar-constrained chat, blocked
external egress, restarted a killed child, unloaded it, and verified signed
import into the store. Arch's package is `llama-cpp` (it provides
`llama-server`), so packaging and the VM script target that name. Remaining work: improve refusal
accuracy with more refusal variants (and likely a larger base model), publish
the first adapter as a signed npack package and fill `registry.lock`, and
pre-install the default base + adapter pair once the Phase 5 ISO profile
exists. Existing disposable model fixtures must be republished with
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

**Adapter training and evaluation (build tooling, not runtime):** The LoRA is
the part that makes operation and tool proposals accurate, so producing it is
part of this phase rather than an external assumption. `ai-inferd` must never
depend on this tooling: it only reads the signed artifact. Training hardware is
opt-in and outside the ISO/build VM; the VM validates the resulting GGUF and
package.

- **M7.18 — Catalogue-derived training data.** Add `tools/adapter-training/`
  with a Rust generator that reads `evermore_common::operations` directly, so
  the dataset cannot drift from the catalogue. Emit deterministic JSONL for
  each operation's name, description, access class, grant, approval class,
  valid arguments, invalid variants, read vs. approval-required write
  proposals, out-of-catalogue refusal cases, and multi-step traces. Synthetic
  catalogue data is the default and needs no owner data. Opt-in local traces
  are an explicit extension: a local export reads the operation journal,
  applies the shared redaction boundary, writes a reviewable JSONL, and
  records its hash; nothing is uploaded automatically. The dataset manifest
  records the catalogue fingerprint, seed, split, counts, and whether
  contributed traces were included.
- **M7.19 — Training and evaluation harness.** Add a reproducible LoRA recipe
  for a 1B–3B base: a versioned PEFT config (rank, alpha, dropout, learning
  rate, epochs, seed) rather than shell history, plus an evaluator over a
  held-out split. Report tool-selection accuracy, argument/schema validity,
  refusal correctness, grammar conformance, proposals for non-existent
  operations, and base-vs-adapter plus adapter-scale (0, 0.5, 1.0) comparisons.
  Write a machine-readable eval report tied to the catalogue fingerprint and
  dataset hash.
- **M7.20 — Versioned adapter release and CI gate.** Record training
  provenance in the model package: base-model hash, dataset hash,
  training-config hash, eval-report hash, catalogue/registry version, and
  whether opt-in traces were included. Pin the expected catalogue fingerprint
  and adapter release in `tools/adapter-training/registry.lock`; a catalogue
  change without a matching release fails the check with an explicit
  "rebuild and re-evaluate the adapter" message. Publishing remains the normal
  signed `npack` flow, and `ai-inferd` still refuses an adapter whose declared
  registry version does not match its manifest.

**Validation:** In the VM, with external networking disabled, install the
signed base and adapter packages, stream chat and embeddings through the
unchanged `ai-inferd` socket, change the adapter scale at runtime, kill
`llama-server` mid-request and confirm the daemon restarts it without losing
its own socket, request catalogue JSON under a grammar, and confirm a
mismatched base/adapter manifest is refused. Confirm the panel and `ai-indexd`
need no protocol changes beyond the version bump.

**Build order:** Manifest and store scan first; supervisor and protocol
translation next; signed import after that. Adapter hot-swap and grammar follow
once baseline chat and embeddings pass. Packaging, ISO pre-install, and the
reinstall path close the runtime work. Adapter training starts from the
catalogue generator and evaluation harness, then a training run, then the
registry lock and signed package.

### Phase 8: Familiar Desktop Experience

**Goal:** Turn the Nostr-aware system services into a conventional, keyboard-
driven Sway desktop. Nostr remains infrastructure for identity, sync, software
provenance, and optional cross-device features; it is not the desktop's visual
theme. A user unfamiliar with Nostr should still recognize the launcher,
notification inbox, settings, workspaces, and system tray. AI is an optional
fallback: when deterministic search can answer a query, no model is invoked.

**Implementation status (2026-11-08):** M8.1 has a tested Waybar preset for
Sway workspaces 1–9, window title, network, audio, battery, clock, and tray.
The current Super+Space launcher is Wofi's XDG desktop-applications mode: it
shows a five-column empty-query grid, accepts first-keystroke fuzzy filtering,
launches the selection, and dismisses on Escape or an outside click. The
input-enabled Arch VM check verified that flow and Waybar startup. Fuzzel stays
installed for deterministic dmenu pickers: its desktop actions expose
searchable Evermore Settings, Notifications, AI, and file search, while the
file-search entry limits results to explicitly indexed scopes. The deterministic
search path does not start inference. `nwg-drawer` was rejected after its
focus handoff consumed the first printable key; Walker/Elephant remains a
comparison candidate rather than a required service. The default launcher is
Super+Space; the stock Super+D binding is disabled without an unbind warning.
The default VM compositor is headless-only and has no input devices, so the
launcher check starts a temporary headless-plus-libinput session and attaches
the VM keyboard. M8.2 v0 and its deterministic desktop-action/file-search
entry points are implemented.

The new `packaging/sway/install-desktop.sh` provides a repeatable XDG user
install for Waybar, Wofi, Fuzzel, SwayNC, Sway bindings, and desktop entries. It
backs up replaced files, does not install packages, and leaves the main Sway
config for the user to include explicitly. An isolated-XDG run verified the
backup and idempotent rerun behavior; Sway parsed the installed includes with a
normal `$mod` definition.

M8.3 uses SwayNotificationCenter for D-Bus toasts, history, grouping, and DND;
the packaged CSS uses open graphite surfaces and unboxed notification rows.
Owner approvals are critical urgency so they surface before ordinary
notifications. Separate Arch VM checks verified a local application toast and
pending owner approval coexist exactly once in the SwayNC control center, and
that the approval's Review action opens the Evermore owner UI and a signed
owner decision completes the request. The status indicator remains in Waybar,
with history in SwayNC.

M8.4/M8.5 are implemented in the existing panel: named sessions are stored
locally with user-only permissions; source grants support one-turn or pinned
scope and snapshot or per-turn refresh; the sanitized context payload and a
conservative token estimate are inspectable, and over-budget requests fail
without silently dropping pinned context. `ai-contextd` now redacts secret-keyed
values as well as secret-shaped strings. The offline search validation exposed
and fixed redaction for spaced assignments such as `password = value`, with
regression coverage for quoted multiword secrets. The panel stream can be cancelled by
closing its inference socket. `--ai` opens a 400 px Sway floating drawer with a
40 px collapsed strip. The Arch VM builds these changes and its Sway parser
accepts the drawer rules. `ai-indexd` filters sibling `.no-ai` markers from
active scopes; `ai-contextd` also rejects a marked file snapshot. Both were
verified with a temporary VM file, including secret redaction. The drawer now
accepts dropped, explicitly selected text files as context and has a
`Super+Shift+V` clipboard-capture binding that fills the prompt without sending.
The standard `org.evermore.SendToAI.desktop` MIME entry opens the drawer with a
selected text file attached for one turn; an Arch VM run confirmed the local
session grant is saved and no message is sent automatically, while a `.no-ai`
file is refused. It is file-manager agnostic and adds no file-manager package
dependency. The input-enabled Sway check also seeded the Wayland clipboard,
sent Super+Shift+V, and visually confirmed the text appeared in the prompt
while inference was unavailable. Panel unit tests cover one-turn context
consumption, pinned-grant retention, snapshot stability versus per-turn
re-resolution, inference-socket cancellation, and the response-reserve budget
boundary.
The Context tray exposes active selected files as internal egui drag sources
and accepts them in the prompt drop zone; a pointer-injected Arch VM check
confirmed that dragging a file into a fresh session creates one unsent
one-turn snapshot grant. External Wayland file drops remain an enhancement.
The drawer now offers context-specific prompt starters for selected files,
workspace, theme, notification, and package context; each only fills the prompt
and still requires the user to send it. Pending operation review cards are
visible in the drawer. When a prompt clearly matches a write operation, the
inference request receives only the five highest-overlap approval-gated write
schemas. A returned `EVERMORE_PROPOSAL` marker renders an inline card with the
validated, canonical arguments; submission requires distinct agent and owner
identities and uses the existing signed request and owner-approval path. Parser,
catalogue filtering, schema rejection, and read-operation rejection have unit
coverage. The card was visually checked from an isolated saved-session fixture,
then validated end to end in the Arch VM with a local Qwen model: the canonical
proposal was submitted by the granted agent, reviewed by the configured owner,
and changed the theme setting only after approval. The AI drawer records its
focused Sway output on open and closes when output focus changes. An expanded
Arch VM check confirms it stays open when another window on the same output
receives focus, opens on the currently focused second output, and closes when
focus then moves back. This validates Sway placement and focus lifecycle across
two headless outputs; final interactive usability review can still refine
sizing and visual placement.

The Nostr feed is a separate right-edge drawer (`evermore-panel
--nostr-feed`, 340×680, `Super+Shift+N`) beside the left-edge AI drawer; each
is its own window and closes on output-focus change. The feed positions itself
against the right edge of the workspace because Sway has no right-anchor rule.
Attaching a note hands it to the AI drawer through a user-private runtime
snapshot and opens the AI drawer if it is not already running. Dragging a note
also copies its text to the Wayland clipboard; a release over the AI window
attaches the note as one-turn context, detected through Sway's window
rectangles because egui cannot drag across native surfaces. The feed requires
configured external relays and explains the missing setting instead of
connecting implicitly.

The `ai-indexd`
search path has been exercised end to
end in a Bubblewrap network-isolated Arch VM using the installed signed BGE
model: a selected file returned for a natural-language query, secrets were
redacted, `.no-ai` removed the scope, and five warm searches had a 4.69 ms
median. This meets the 50 ms semantic-search budget for this model and VM. The
selected-file picker now keeps exact path selection deterministic and sends an
unmatched query to the semantic search service; its distinct Fuzzel result
picker was visually checked in the input-enabled VM. Its first attempt exposed
that Fuzzel discards UTF-8 dmenu rows under the VM's POSIX locale and omits
overlong display fields. The picker now uses a UTF-8 character locale and
bounded filename/excerpt fields. The file-search launcher now adds a distinct,
copyable local suggestion for open-ended queries only when inferd reports a
chat-capable model already loaded. The request is warm-only, creates no saved
chat, and is omitted when the model is cold or unavailable. An Arch VM test
verified the deterministic semantic results remain selectable, the separate
suggestion row copies its response, and the request carries `warm_only=true`.
The AI fallback is scoped to the file-search launcher flow and does not create
a general chat surface. Once a warm model produces its first token within
300 ms, the visually distinct suggestion row appears while generation
continues; Escape or selecting a file cancels the remaining stream, and
selecting the suggestion waits only within the 3 s total budget. The launcher
suppresses a cold or late suggestion. VM acceptance now
checks copy and Escape cancellation using a fake warm model, and a separate
timed-socket test verifies a stalled model-list request returns within budget.
A VM timing run with the installed
TinyLlama chat model measured 1.65 s to first token on a cold request, 25.7 ms
on the next warm request, and 703 ms total for the warm response. The warm
request meets the 300 ms first-token and 3 s completion budgets; cold start does
not. The launcher consequently hides the suggestion unless a chat-capable
model is already warm. The input-enabled VM verified that the suggestion is
separate from deterministic results, copies its response, and cancels generation
on Escape. No custom general-purpose launcher or layer-shell toolkit has been
added.

**Phase 8 acceptance (2026-11-08):** The current Arch VM run passed the panel's
12 unit tests; Send-to-AI and `.no-ai` rejection; same-output and multi-output
drawer focus; notification deep-link and signed owner approval; local-plus-
approval inbox deduplication; scoped semantic search and its 50 ms target; Wofi
empty-grid, first-key launch, Escape, and outside-click behavior; and both warm
AI suggestion and Escape-cancellation modes. The real-model proposal check
generated the canonical `settings.set` arguments, submitted a signed pending
request, and changed the theme only after the configured owner approved it.
These checks use an isolated headless Sway session in the input-enabled Arch
VM and disposable identities/state. External Wayland file drops and final
interactive sizing review remain documented enhancements; the required
file-manager handoff and internal context drag paths are tested.

The index, inference, context, and Sway bridge user units now create their
shared 0700 runtime directory. The index service also has read-only home access,
a private service-owned state directory, and Unix-socket-only networking. The
Arch VM verified unit parsing, indexd and contextd startup/health responses,
inferd model discovery, and 0700 state / 0600 socket permissions. Index state
uses a dedicated directory to avoid systemd's legacy config-to-state
compatibility link.

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
- **Launcher:** use [Wofi](https://man.archlinux.org/man/wofi.1.en) in drun
  mode for the five-column application grid and keyboard search; the empty
  grid, immediate typing, launch, Escape, and outside-click behavior pass in
  the VM. Keep [Fuzzel](https://codeberg.org/dnkl/fuzzel) for deterministic
  dmenu-based file and desktop-action pickers, not as a second default binding.
  Keep system mutation in Evermore's catalogue and retain
  `evermore-panel --command-palette` as the safe Evermore action surface until
  it can be integrated cleanly as a launcher provider/menu.
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
- **M8.2 — Launcher v0, then v1.** Configure upstream Wofi in drun mode for
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

### Phase 9: Offline Distribution Image and Installer

**Goal:** Turn the assembled components into a machine. A bootable `archiso`
ISO installs a complete Evermore system with no network access, boots to a
greeter, provisions the first human identity from a pubkey at first boot, and
can install signed software through `npack` from a trust list signed by the
device's own root identity.

**Implementation status (2026-11-08):** Not started. This phase absorbs M5.6
(the archiso profile), applies the Phase 5.5 identity decisions, consumes the
Phase 7.5 model store, and wraps the Phase 8 desktop configuration. It also
settles M6.5's release-shape question for the first release: the ISO — not AUR
packages — is the deliverable, with packages for stock Arch remaining a
secondary path for contributors and dogfooding. The four scope decisions below
are made; the rest of the phase is plan.

**Decided scope:**

- **Offline first.** The ISO carries a signed local package repository and the
  pre-extracted default model pair, so install, first boot, and local inference
  all work with no network. Networking is needed only afterwards, to reach
  relays and Blossom servers.
- **The installer creates no human user.** A device exists before a person does.
  Human accounts are `systemd-homed` records created at first boot, consistent
  with Phase 5.5's "the pubkey is the identity." No `useradd`, no sudo group,
  no pre-seeded home directory.
- **Per-home LUKS2 and systemd-boot with UKIs.** Each identity's home is
  encrypted by `systemd-homed`; root is unencrypted by default. UKIs
  (`systemd-stub`) are the boot artifact so TPM2/FIDO2 enrollment stays
  available later. Secure Boot remains optional and is never a requirement.
- **The model pair ships on the ISO.** The Phase 7.5 `Q4_K_M` base and `F16`
  adapter are pre-extracted into `/var/lib/evermore/models` with their signed
  manifests and release events, rather than pulled on first boot. This is what
  makes an offline first boot meaningful, at the cost of the device-root
  at-rest problem recorded below.

**Milestones:**

- **M9.1 — Installable binary and unit layout.** Move every unit's `ExecStart`
  off `%h/.cargo/bin` and `%h/.local/bin` onto `/usr/bin`, ship the user units
  in `/usr/lib/systemd/user` with `default.target.wants` symlinks so they
  enable for every homed user, and add an `evermore-desktop` metapackage that
  depends on the Sway/Waybar/Wofi/Fuzzel/SwayNC stack, `systemd-homed`, `greetd`,
  pipewire, polkit, and the desktop portal. `packaging/sway/install-desktop.sh`
  becomes a post-install step rather than a README instruction.
- **M9.2 — Signed offline repository.** `cargo build --release` for the daemons
  and `go build` for `evermore-relay-supervisor` (it already embeds khatru, so
  the target needs no Go toolchain), then `repo-add` into a `[evermore-local]`
  repository signed by an ISO build key whose public key ships in the live
  keyring. No `SigLevel = TrustAll`, and no `cargo` on the target image.
- **M9.3 — `archiso` profile.** `packaging/archiso/` with `profiledef.sh`,
  `packages.x86_64`, and an `airootfs/` overlay: a `pacman.conf` pointing at
  the bundled repo, the greetd configuration, first-boot and enrollment units,
  the pre-extracted model store, and the seed trust list. `copytoram` off;
  roughly 4–6 GB expected.
- **M9.4 — `evermore-install`.** A `dialog` TUI with a JSON answers file for
  unattended runs. A 1 GiB ESP mounted at `/efi` plus root on the remaining
  space; `pacstrap` against the ISO's `pacman.conf` so dependency resolution is
  fully offline; `genfstab -U`; chroot configuration; `bootctl install` with
  `/etc/kernel/cmdline` using `root=PARTUUID=…` and mkinitcpio's
  `default_uki=/efi/EFI/Linux/evermore-linux.efi`; enable `systemd-homed` and
  `greetd`; copy the model store. Deliberately creates no user.
- **M9.5 — Device-root first boot.** `evermore-firstboot.service`, gated on the
  absence of `/var/lib/evermore/device/root.npub`: generate the device-root
  keypair, sign `var/lib/evermore/trust/seed.json` into the starter NIP-51
  trust list, and seal the key according to the at-rest decision below. This
  needs a **system-context signer instance**, a deployment shape Phase 1 does
  not have (its signer is a user unit) — new work, not configuration.
- **M9.6 — First-boot enrollment.** `evermore-enroll-tui` on tty1 while
  `homectl list` reports no users: collect an npub or mint a new identity,
  choose custody (local `ncryptsec` inside the encrypted home, or NIP-46
  pairing through `evermore-pkcs11` once that provider exists), write the
  identity JSON with its `"evermore"` block, call `evermore-enroll create`,
  then hand tty1 to `greetd`. Later boots skip this and run `tuigreet --cmd
  sway`. This is the largest genuinely new piece: `greetd` has no enrollment
  UI, and the greeter cannot discover an identity that does not exist yet.
- **M9.7 — First-login inheritance.** On the first successful login, import the
  device-root signing floor into the user's `evermore:trusted-publishers`
  NIP-51 list, seed `evermore:relays`, and start the user units through
  `default.target.wants`. **No `loginctl enable-linger`**: lingering outlives
  the PAM session and revokes the session keyring the signer depends on, which
  is the `KeyRevoked` failure already recorded in `vm/README.md`.
- **M9.8 — Session dependencies.** Install and configure the pieces the
  desktop silently assumes: a polkit authentication agent,
  `xdg-desktop-portal` plus `-wlr`, fonts and an icon theme, pipewire with
  wireplumber, and `nmtui` in the live environment. Establish the notification
  rule once: SwayNC is the sole `org.freedesktop.Notifications` owner and
  `evermore-notifyd` bridges into it, never claims the name.
- **M9.9 — Install acceptance harness.** `vm/install-test.sh` boots the ISO
  against a scratch overlay disk with `-net none`, runs the unattended install,
  reboots, enrolls a fixture identity, and asserts the result. It is a
  separate target from `vm/create-vm.sh`'s dev VM and becomes the gate for
  every later ISO change.

**Validation:** Build the ISO with `mkarchiso` and boot it in a fresh VM with
no network device at all. Install unattended, reboot, complete first-boot
enrollment, log in through greetd, and confirm every user unit reaches
`active`, the signer holds the identity, the device-root seed appears in the
user relay, `npack` installs a bundled package signed by a trusted publisher,
and `ai-inferd` loads the on-ISO model pair with egress blocked. Repeat once
with a TPM2/FIDO2 factor attached to confirm the UKI still boots and homed
still unlocks.

**Build order:** M9.1 and M9.2 first (prebuilt binaries and the offline repo),
then M9.3 and M9.4, then M9.5–M9.7, then M9.8, with M9.9 hardening alongside.
Prototype M9.5/M9.6 in the dev VM before the ISO is complete: "no users exist
at first boot" is where `greetd`, homed, and the trust seed all meet, so it is
the part most likely to invalidate the rest of the plan.

**Open risks:**

- **Per-home-only LUKS leaves the device-root key unprotected at rest.** Root
  is unencrypted, so `/var/lib/evermore/device/` is readable from an offline
  disk. Seal the device-root `nsec` with `systemd-creds`/TPM2 under a PCR
  policy, or encrypt root after all, before M9.5 is considered finished.
- **Custody default for the first identity is unresolved.** Local `ncryptsec`
  and NIP-46 differ in first-boot UX and failure modes; Phase 5.5 leaves the
  choice open, and M9.6 cannot.
- **ISO size.** A local repo plus roughly 1 GB of models lands near 4–6 GB.
  Keep `copytoram` off and verify the image still fits its intended media.
- **`/home` must support sparse files** for homed's LUKS2 home areas, and
  `/var/lib/systemd/home` must stay on persistent root so identity records
  survive.
- **Two package managers, one filesystem.** pacman owns base and upstream
  packages; `npack` owns Evermore applications and models and links into
  `~/.local/bin`. PATH precedence and file-conflict rules belong in this
  phase, not discovered later.

### Phase 10: Desktop Concept — Launcher and Panel

**Goal:** Evolve the proven Sway desktop into the familiar, keyboard-first
concept in the desktop brief. The launcher is a one-shot invocation surface;
Waybar remains passive system status. Nostr stays plumbing. Launcher reads
remain deterministic by default, and actions that change system state continue
through the Phase 7 operation registry and owner-approval path.

**Implementation status (2026-11-08):** This phase starts from the tested
Phase 8 implementation rather than replacing it. Waybar uses its upstream
Sway workspace, mode, title, network, audio, battery, clock, and tray modules.
Super+Space now opens Wofi's XDG desktop-applications mode: its five-column
empty-query grid, fuzzy first-key search, launch, Escape, and outside-click
dismissal passed in the input-enabled Arch VM. Wofi is already in Arch's
official `extra` repository, uses wlroots layer-shell, and needs no resident
provider process ([manual](https://man.archlinux.org/man/wofi.1.en)). Fuzzel
remains for Evermore's deterministic dmenu pickers. This closes the app-grid
gap without custom launcher or layer-shell client code. Explicit
favourites are still missing; Wofi's cache is not treated as a pinning or MRU
contract until that ordering is separately validated.

**Reuse assessment:**

- **Waybar remains the panel.** Its built-in Sway, clock, tray, audio, network,
  and battery modules cover the whole conventional status bar. Waybar also has
  [native module-group drawers](https://github.com/Alexays/Waybar/blob/master/man/waybar.5.scd.in#L1730)
  with click or hover reveal and slide transitions;
  these compact groups live inside the status bar, not on screen edges. Use
  them if usability calls for a compact status cluster. Keep the tested
  default expanded and readable until then. This avoids writing status
  collectors or a panel framework.
- **Wofi is the current grid launcher.** Its `drun` mode reads XDG `.desktop`
  entries, `columns=5` gives an empty-query grid, fuzzy matching handles typed
  queries, Tab expands desktop actions, and it passed the VM focus/launch
  sequence. It is in Arch `extra` and has no resident service. Its cache puts
  previously selected entries first, but it has no explicit pin/unpin actions.
  Keep the default mode restricted to desktop applications. Fuzzel remains the
  established choice for dmenu-based deterministic pickers and a fallback
  candidate ([Wofi manual](https://man.archlinux.org/man/wofi.1.en),
  [Fuzzel manual](https://man.archlinux.org/man/fuzzel.1.en)).
- **Walker + Elephant is the strongest reuse candidate for pinned apps and
  richer providers.** The desktop-applications provider documents history-
  aware ranking, favourites/pinning, desktop actions, and empty-query results;
  [Walker](https://walkerlauncher.com/docs/providers) also supports a resident
  service and custom layouts. It adds the
  Elephant service and separately installed providers, however, and the Arch
  VM has no official `walker` or `elephant` package. It is a list-oriented
  frontend, so it does not yet prove the app-grid requirement. Evaluate it in
  an isolated VM before changing the default or adding it to the ISO package
  set.
- **Do not revive `nwg-drawer` as the keyboard launcher.** The earlier VM
  focus test showed its app-tile-to-search handoff consumes the first typed
  character. It may still be used as a mouse-only grid prototype, but that
  would add a second surface and would not satisfy the keyboard-first flow.
- **Build a custom app surface only for the remaining product-specific gap.**
  If Wofi or another packaged launcher cannot provide explicit favourites,
  recent ordering, searchable desktop actions, and reliable immediate keyboard
  focus in one surface, prototype only that frontend in the existing egui
  application.
  Keep `.desktop` parsing and launch semantics in existing desktop standards;
  keep Evermore settings and operation actions on their existing catalogue
  paths. Add only the Wayland layer-shell client needed for that frontend.
  Start with instant show/hide; Waybar already supplies animated drawers for
  its own status modules, and SwayFX remains an optional compositor profile.

**Milestones:**

- **M10.1 — Launcher and Waybar baseline.** Keep the tested Wofi drun
  binding on Super+Space and the current Waybar modules. Make the Waybar
  workspace 1–9, window title, notification count, clock, and tray behavior
  visually coherent with the desktop brief. Do not add protocol status to the
  default panel. The baseline is implemented; retain its VM acceptance as the
  regression gate for future launcher work.
- **M10.1 validation (2026-11-08):** Re-ran the input-enabled Arch VM check.
  Super+Space opened Wofi, the first typed character filtered applications,
  Enter launched Foot, Escape dismissed the launcher, and an outside click on
  the same output dismissed it. The same run verified the deterministic to
  semantic file-search handoff and clipboard hotkey. Sway started Waybar from
  the packaged config; a VM screenshot showed workspaces 1–9, network, inbox
  count, and clock. Audio, battery, window title, and tray had no visible
  values in the headless test session. Set
  `EVERMORE_PHASE8_WAYBAR_SCREENSHOT` to preserve this screenshot during future
  runs of `vm/phase8-launcher-input.sh`.
- **M10.1 launcher update (2026-11-08):** Wofi replaces the Fuzzel drun
  binding while Fuzzel stays installed for dmenu pickers. The new
  `vm/phase10-launcher-waybar.sh` check passed in an isolated headless Sway
  session with input: Super+Space displayed the five-column app grid, the
  immediate `foot` query filtered and launched Foot, Escape closed the grid,
  and an outside click dismissed it. The check launches Waybar from the
  packaged config. Explicit pin/favourite ordering and comparative summon
  latency remain open, so M10.2/M10.3 are still in progress.
- **M10.1 visual pass (2026-11-08):** The launcher and panel now share the
  same charcoal, cool-blue, and warm status palette. Waybar keeps the
  required workspaces, window title, network, inbox count, clock, audio,
  battery, and tray modules, with clearer active-workspace, hover, tooltip,
  and warning states. Wofi keeps its native five-column grid and desktop
  actions. The acceptance script now captures a Waybar-only image before
  opening Wofi, separately from the app-grid image. No custom launcher,
  drawer, or layer-shell code has been introduced; Waybar's built-in module
  group drawer remains available if a compact status cluster proves useful.
- **M10.1 graphite shell pass (2026-11-08):** Waybar uses native module
  groups for workspaces/mode, active window title, and system status. Its
  visual hierarchy comes from spacing, text contrast, and an underline on the
  active workspace; no modules are enclosed in outlined islands. Wofi uses a
  single soft floating surface with transparent app entries, a search
  underline, and underline selection. The existing input-enabled VM check
  passed after the visual change: bar startup, empty grid, first-key filtering,
  app launch, Escape, and outside-click dismissal. The screenshot review
  confirms there are no repeated module or app-tile boxes; the Waybar title
  still needs a foreground-window screenshot.
- **M10.2 — Reuse candidate spike.** Compare Wofi, Fuzzel, and Walker/Elephant
  against the same local application set for summon latency, first-keystroke focus,
  empty-query behavior, usage ordering, pin/unpin, desktop actions, and
  dependency cost. Keep provider sets restricted to installed desktop apps
  plus explicitly approved Evermore menus; omit command-runner and web-search
  providers. Adopt Walker only if its additional service and package source
  fit the target distribution and its interaction passes the VM check.
- **M10.2 reuse finding (2026-11-08):** Wofi is the practical first grid:
  the Arch VM has Wofi 1.5.3 from the official repositories and its drun flow
  passes the keyboard acceptance check. Fuzzel 1.15.0 remains the lean,
  previously tested list launcher and powers deterministic pickers. Walker's
  upstream `desktopapplications` provider offers history ranking, pin/unpin, desktop
  actions, and empty-query results, but the published frontend is organized
  around result lists and does not establish the required app-grid view. Its
  current Arch installation instructions use the AUR and add the Elephant
  service plus provider packages; `pacman` found no official Walker or
  Elephant package in the VM. This makes it a poor default-distribution fit
  without a separate packaging decision. The full M10.2 timing and interaction
  comparison remains open; do not claim Walker's grid behavior or latency has
  been validated.
- **M10.3 — App grid and favourites.** Wofi now supplies the empty-query grid
  and cached entries ahead of the rest. The remaining requirements are explicit
  pinned favourites, verified recent ordering, and alphabetical ordering for
  the other apps. Typed queries switch to ranked results without losing the
  first keystroke.
  Enter launches and dismisses, Escape cancels, and desktop actions remain
  available. Reuse an upstream frontend if it meets this whole interaction;
  otherwise implement only this frontend using the existing egui stack. Never
  create a generic command-execution path.
- **M10.4 — Settings and operation entries.** Add deterministic settings-pane
  and safe application actions to the same invocation surface, through XDG
  desktop actions or a narrow Evermore menu integration. Mutating actions
  must resolve through the Phase 7 catalogue and owner-approval flow. Preserve
  direct application launching as the zero-daemon path.

**Validation:** In the Arch VM, start from the installed desktop config and
measure summon-to-first-result time. Check empty-query display, pinned and
recent order, typing the first character immediately, desktop actions, launch,
Escape, outside-click/focus dismissal, and behavior with optional services
stopped. For Waybar, check workspaces 1–9, window title, notification count,
clock, tray, audio, network, and battery where hardware exists. If a group
drawer is enabled, validate both click-to-reveal and its transition with the
keyboard and pointer. No inference service may start for ordinary app or
settings searches.

**Build order:** Preserve the working Wofi/Waybar baseline; finish the reuse
comparison and explicit pin/recent behavior; add settings actions after the
basic invocation flow is stable. Side drawers and animation work follow only
when their own surface usability is tested.

## Key Design Decisions and Risks

A few decisions will shape the entire project and are worth resolving early:

- **Trust bootstrapping for `npack`.** The system needs a usable default trust set on first boot, or the package manager is a blank slate. Decide whether the ISO ships with a curated publisher list, a "trust on first use" model, or a hybrid. The `npack` project's explicit "no central repository" design means *you* must provide the initial social graph seed. Phase 9 settles the mechanism without settling the contents: a starter list is baked into the image as data, signed at first boot by that device's own root identity, and inherited by each human identity as an extendable floor — so the seed is per-device and revocable, not a distribution key compiled into every binary.
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
runtime, packaging, training, and VM checks are implemented, and adapter
hot-swap has been exercised end to end; the signed adapter release and ISO
default pair remain. It does not change Phase 7's acceptance evidence, but it
must complete before Phase 8's AI drawer and semantic launcher fallback are
considered complete.

## Addendum: Application Set — Build vs. Reuse (Design Note)

**Status:** design note, not a phase. It records a policy for choosing the
default application set; it adds no milestones, services, or acceptance gates.
Revisit it when a concrete app decision arises, and record the outcome wherever
that app is actually specified.

**Question.** If Evermore is a Nostr+AI distribution, which applications belong
in the default image, and which should Evermore write itself?

**Answer.** Almost all apps should be stock Linux applications with one narrow
integration hook. Evermore-authored UI should be the control plane — identity,
settings, approvals, software, notifications, and AI context — not
productivity software. This follows directly from the architecture rule that
integration lives in headless daemons, not in individual applications
(see "The Revised Architecture"). The app count should stay small; the long
tail arrives through `npack`, not through the ISO.

### The filter

An app warrants custom development only if at least one of these is true:

1. **It owns identity, trust, approval, or audit semantics.** No upstream app
   can, because the key boundary and the operation catalogue are Evermore's.
2. **There is no mature upstream.** Examples: `npack`, the operation
   catalogue, context grants, the local model store.
3. **The AI-plus-context workflow is the product**, and it needs cross-app
   plumbing — scopes, redaction, token budgets, approval cards — that a stock
   app cannot expose.

If none apply, reuse the app and give it exactly one hook: a `.desktop` action,
a MIME handler, a CLI wrapper, the signer socket, or a NIP-46 pairing. Do not
fork an app to add a second integration protocol, and do not let one app's
special case become a framework.

### App set by tier

| Tier | App | Recommendation | Rationale |
| :--- | :--- | :--- | :--- |
| **Shell** | Sway, Waybar, Wofi (grid) and Fuzzel (dmenu pickers), SwayNC, swaybg, swaylock, swayidle, grim/slurp, wl-clipboard, xdg-desktop-portal-wlr | **Reuse**, as already chosen in Phases 8 and 10 | Generic surface; Evermore's value sits behind it, not inside it |
| **Login** | greetd with gtkgreet or regreet | **Reuse**; custom only the identity picker if Phase 5.5 requires it | PAM/Nostr logic belongs in the module, not the greeter |
| **Control plane** | `evermore-panel`: settings, notifications, approvals, AI drawer, file search | **Custom, egui** — exists | This is the Evermore product surface |
| **Identity** | Account and device manager: NIP-46 pairing, NIP-49 recovery, per-app grants, trust/relay/Blossom lists, audit log | **Custom, egui** (dedicated screens, possibly inside the panel) | No upstream exists; it is the core UX of the distribution |
| **Software** | `npack` software center and model manager | **Custom, egui** (or a panel pane) | No analogue exists; small, dense, stateful UI, a good egui fit |
| **Onboarding** | First run: create or import identity, seed the trust list, choose relays, download a model | **Custom, small** | Distro-specific and unavoidable; also where the trust-bootstrap decision becomes visible to the user |
| **Files** | Nautilus (or Thunar or yazi) plus `Publish to Nostr`, `Send to AI`, and the Blossom mount | **Reuse plus scripts/MIME handlers** | The pattern already exists in `packaging/nautilus`; do not write a file manager |
| **Terminal** | foot | **Reuse** | Wayland-native and minimal |
| **Editor** | helix, kate, or gnome-text-editor | **Reuse** for plain text | egui is not a document or rich-text editor |
| **Browser** | Firefox (or Chromium) plus a **NIP-07 to `evermore-signer` native-messaging bridge** | **Reuse the browser; custom bridge** | The highest-leverage integration in the set: web Nostr apps use the system signer instead of browser-local keys |
| **Nostr social and DMs** | gossip or notedeck (both Rust/egui), paired through NIP-46; or a thin inbox over `notifyd` | **Reuse or fork and integrate — do not build a feed client** | Social UX is open-ended; Evermore's differentiator is keys, notifications, and triage, not timelines |
| **Media and viewers** | mpv, imv or loupe, zathura, file-roller, a calculator | **Reuse** | Commodity; one "Send to AI" action is enough |
| **Capture** | grim/slurp, wf-recorder, the capture hotkey | **Reuse plus the existing hook** | Already part of M8.6 |
| **Mail and office** | Thunderbird, LibreOffice — optional, not default | **Reuse if shipped** | Not the product; do not spend image space or attention on it |
| **AI** | The panel drawer; **no standalone chatbot app** | **Custom, exists** | A generic chat app adds nothing; context capture and approval are the product |

### Custom shortlist, in build order

1. **Panel** — exists; keep it the home for settings, inbox, approvals, and
   file search.
2. **AI session drawer** — exists; resist splitting it into a separate "AI
   app". The drawer plus context entry points is the whole AI surface.
3. **Identity and device manager** — the largest missing piece and the most
   clearly Evermore's own. It is where NIP-46 pairing, recovery, per-app
   grants, and the audit trail become usable without a terminal.
4. **`npack` software center and model manager** — a small UI over an API
   Evermore already controls. Model installation is a first-class path, not a
   hidden CLI, because local inference is part of the product.
5. **First-run onboarding** — required by any real image, and the natural
   place to explain identity, trust seeding, and the optional nature of Nostr
   sources.
6. **Conditional, in this order:**
   - App-grid launcher with pinned favourites — only if Wofi and Walker fail
     the M10.2/M10.3 comparison.
   - Inbox and triage surface — only if SwayNC cannot group sources and
     surface pending approvals.
   - A Nostr-native **Notes** app — only if note-taking is a product pillar.
     It is the one plausible flagship custom app beyond the control plane, and
     it should be built on NIP-23 and the existing context/AI services, not as
     a new editor stack.

Items 1–5 are all one category: system and control surfaces. That is the
honest scope of egui in this distribution, and it matches the existing
"`smithay-egui` is not a shell" decision.

### Integration hooks instead of forks

The default app set should share one integration contract:

- **Launch and context:** `.desktop` actions, MIME handlers, and the
  `org.evermore.SendToAI` entry, so a file or selection reaches the AI drawer
  without a per-app plugin.
- **Signing:** NIP-46 for apps that speak it; the `evermore-signer` socket for
  Evermore's own clients. No app receives the raw key.
- **Web:** a native-messaging host that exposes the signer to a browser
  extension as a NIP-07 provider. This keeps web Nostr clients usable without
  importing an `nsec` into the browser.
- **Files:** the Blossom mount and the Nautilus script; other file managers get
  the CLI equivalent.
- **System status:** Waybar modules and D-Bus notifications; no app publishes
  its own panel widget or notification channel.
- **Packages and models:** `npack` plus the software center; apps do not
  self-update outside it.

### What not to build

A browser, a terminal, a file manager, an office suite, a mail client, a media
player, an image or PDF viewer, an archive manager, a generic chat client, a
general social feed, a generic AI chatbot, a desktop shell, or a widget
toolkit. Each has a mature upstream, and each would compete for attention with
the control plane that is actually differentiated.

### egui suitability

- **Strong:** dense panels, settings forms, status and audit lists, operation
  cards, streamed responses, drag-and-drop context trays, and fast iteration.
  This is exactly the control plane.
- **Weak:** rich text and document layout, complex-script and RTL shaping,
  accessibility (AccessKit support exists, but Linux screen-reader coverage is
  thin), and long-form text editing. These weaknesses are a further reason to
  reuse GTK or Qt apps for text-heavy surfaces and keep egui for control UI.
- **Layer shell:** still needs one small integration per surface, as Phase 8
  notes. It must not grow into a toolkit or a shell project.

### Open decisions

- **Nostr client stance.** Ship an existing client (gossip or notedeck) with
  NIP-46 pointed at `evermore-signer` and accept its key-handling gaps, or
  build a minimal egui client limited to Evermore's workflows. Everything else
  in the table is largely forced by the architecture; this one is a genuine
  product choice.
- **Notes as a pillar.** Whether note-taking is a flagship app or just another
  `npack`-installable package decides whether item 6 above becomes real work.
- **Target user.** A technical early adopter justifies the small default set
  and the reliance on `npack`; a broader audience would require more bundled
  apps and much stronger accessibility, which changes the egui scope.
- **Browser and bridge scope.** Whether the NIP-07 bridge ships with the
  default browser or is an optional package affects both the trust story and
  the maintenance burden.

## Addendum: Visual Design System — Evermore Graphite (Design Note)

**Status:** design note, not a phase. It defines the visual language, tokens,
and layout system shared by the desktop surfaces, and records an
implementation order that keeps them coherent. It adds no milestones,
services, or acceptance gates; it refines how the surfaces specified in
Phases 8 and 10 should look and fit together.

**Scope:** Waybar, Wofi and Fuzzel, SwayNC, `evermore-panel` in every mode,
Sway window chrome, and the greeter, lock screen, and boot surfaces. It does
not change the build-vs-reuse decisions in the application-set addendum above.

### Direction: "quiet instrument"

Dark-first, low-chroma graphite neutrals, a single cool-blue accent, amber
reserved for attention, red for destructive actions. Interfaces use typography,
spacing, and subtle tonal changes to establish hierarchy. Sections separate
from the page with a soft `ink-850` fill and no stroke — a tonal shift, not a
drawn border — rather than relying on whitespace alone; nested containers and
repeated hard outlines are still avoided. Reserve actual border lines (the
`line` token) for visible keyboard focus and controls whose edges aid use;
shadows appear only on floating surfaces. Focus is always visible because the desktop
is keyboard-first. The default desktop must not advertise Nostr or AI: protocol
state belongs in settings and the inbox, following the Phase 8 product rules.
Motion is feedback, never decoration.

The direction formalizes the palette that the Waybar, Wofi, and Fuzzel
configuration already drift toward (`#20242b`, `#8bb7ff`). The panel's purple
accent (`#5A46DC`) is the outlier to remove.

### Design tokens

The palette currently lives in six hand-maintained files and has already
drifted. One token file becomes the source of truth:
`packaging/theme/theme.toml`, with a small `tools/theme-gen` binary that emits:

- `packaging/waybar/style.css`
- `packaging/wofi/style.css`
- `packaging/swaync/style.css` (new; SwayNC sets `ignore-gtk-theme: true`
  today and has no stylesheet)
- the `[colors]` section of `packaging/fuzzel/fuzzel.ini`
- `packaging/sway/evermore-theme.conf` for Sway client colors and gaps
- the panel's theme constants, embedded at compile time
- GTK `settings.ini` for font, icon theme, and cursor, plus foot colors

`theme-gen --check` runs in the test suite so generated files cannot drift
from the tokens. This is the same "one policy, many projections" pattern the
daemons already use.

Palette, with contrast ratios computed against `ink-900` unless noted:

| Token | Hex | Contrast | Use |
| :--- | :--- | :--- | :--- |
| `ink-950` | `#0E1014` | — | Application canvas, lock screen |
| `ink-900` | `#14171D` | — | Waybar, panel chrome |
| `ink-850` | `#1A1E25` | — | Cards, list rows |
| `ink-800` | `#222731` | — | Inputs, raised surfaces |
| `line` | `#2C333F` | 1.41:1 | 1 px borders; decorative, not a text boundary |
| `text` | `#E6E9EF` | 14.8:1 | Primary copy |
| `text-muted` | `#A7B0BF` | 8.2:1 | Secondary copy |
| `text-faint` | `#868F9E` | 5.2:1 | Tertiary copy and timestamps; adjusted from `#7C8595` to clear AA on cards |
| `accent` | `#6EA8FE` | 7.4:1 | Selection, links, focus base, press/active |
| `accent-strong` | `#8BB7FF` | 8.8:1 | Hover, focus ring |
| `accent-ink` | `#0B1020` | 7.8:1 on accent | Text on accent fills |
| `success` | `#5FBF8F` | 7.4:1 | Confirmed states only |
| `warning` | `#E5B567` | 8.9:1 | Modes, attention, pending approvals |
| `danger` | `#E06C75` | 5.2:1 | Destructive, critical, errors |

Color usage rules:

- **Blue means interactive.** Selection, focus ring, active workspace, primary
  button fill with `accent-ink` text, and links. Never a large background
  wash.
- **Amber means attention, not error.** Sway mode indicator, pending approval
  border, battery warning.
- **Red means irreversible.** Delete, deny, critical battery, disconnected
  network.
- **Green is rare.** Only signed, installed, or synced confirmation; never
  decoration.

Non-color tokens:

| Token | Value |
| :--- | :--- |
| Base unit | 4 px; all spacing is a multiple |
| Spacing scale | 4, 8, 12, 16, 24, 32 px |
| Radius | 6 px inputs, 10 px cards, 14 px windows and tiles |
| Border | 1 px `line` for controls; not for repeated cards or tiles |
| Shadow | `0 8px 24px rgba(0, 0, 0, 0.35)`, floating surfaces only |
| Focus ring | 2 px `accent-strong` |
| Motion | 120 ms hover, 180 ms expand, 240 ms drawer content; `cubic-bezier(0.2, 0, 0, 1)` |

A light variant now exists in `evermore-panel` as a first draft (no
corresponding Waybar/Wofi/SwayNC build yet): white canvas/chrome (`#FFFFFF`),
card fill `#DFE2E7`, primary text `#14171D`, muted text `#5A6270`, same
accent/warning/danger hues as dark. The canvas-to-card gap is deliberately
much wider in absolute RGB terms than `ink-900`-to-`ink-850` was for dark —
equal perceived contrast needs a bigger numeric delta at high luminance than
at low luminance, so reusing the dark gap read as almost flat. High-contrast
variants remain out of scope; the token layer still expresses both themes
instead of hardcoding one.

### Typography

- **UI:** Inter (OFL), weights 400/500/600, bundled into the panel and
  installed system-wide for GTK surfaces and the bar.
- **Mono:** JetBrains Mono (OFL) for the terminal, code, and key hints.
- **Fallback:** Noto Sans for script coverage. The custom egui surfaces are
  Latin-first and must not claim RTL or complex-script support.
- **Scale:** 11 px caption, 12 px small, 13 px body, 15 px subtitle, 18 px
  title, 24 px display; line height 1.45.
- **Panel:** load fonts through `FontDefinitions` with embedded bytes, set
  `TextStyle` sizes once, and expose a `font_scale` setting from 1.0 to 1.5.
  Do not depend on system font discovery for the panel.
- Waybar 12.5 px; Wofi 13 px; SwayNC 13 px body with 12 px metadata.

### Iconography

- **UI icons:** Lucide (ISC) at 24 px with a 2 px stroke for rail items, list
  rows, and toolbar actions. Use an icon-font integration compatible with the
  pinned egui version; `egui-phosphor` is the fallback if Lucide's font
  packaging is inconvenient. One set only, and no emoji as UI iconography.
- **Status bar:** Nerd Font glyphs (JetBrains Mono Nerd Font) with an icon and
  a short value. Keep a text-only option for clarity and accessibility.
- **Application icons:** Papirus-Dark as the system theme. The two Evermore
  `.desktop` files gain `Icon=`, `StartupWMClass`, and `Keywords=`; their SVGs
  install under `$XDG_DATA_HOME/icons/hicolor/scalable/apps/`.
- **First-party marks:** Panel, AI, Packages, and Files, drawn on the same
  24 px grid and 2 px stroke as Lucide so they sit together.
- **Cursor:** Bibata Modern Ice at 24 px.
- **Wallpapers:** three dark variants (graphite, dusk, frost) at 2560×1440
  plus an ultrawide crop, applied with `swaybg -m fill`.

### Layout system

One set of dimensions shared by every surface:

| Token | Value | Notes |
| :--- | :--- | :--- |
| Rail width | 72 px icon-only, 220 px labelled | Switches at a 760 px window width |
| Header height | 56 px | Title, description, and primary action |
| Content padding | 20 px, 24 px above 1000 px | |
| Surface radius | 14 px floating surfaces; 6 px controls | Avoid boxed list rows and tiles |
| Control height | 32 px; 28 compact, 36 comfortable | |
| Row height | 40 px; 44 px for two-line list rows | |
| Section gap | 28 px | |
| Maximum text measure | 72 characters | Settings and descriptions |

Density is a `comfortable` or `compact` setting that changes control and row
heights, not type size; font scale is separate.

#### Panel shell

Full mode is 900×640 with a 760×520 minimum. Below 760 the rail collapses
from 220 px with labels to 72 px icon-only and labels become tooltips. Below
560 the panel presents as the drawer. The other modes keep their own sizes:
AI drawer 400×680, command palette 720×520, approval window 620×460.

```
┌──────────────────────────────────────────────────────────────┐
│  ◉ Identity    Settings                              Save    │
│  ⚙ Settings    Theme, relays, trust, and local data          │
│  ◈ Notifs      ───────────────────────────────────────────   │
│  ✦ AI          APPEARANCE                                    │
│  ▣ Packages    Theme        Graphite  ▾                      │
│                Accent       ●  ●  ●  ●  ●                    │
│                Density      Comfortable  ▾                   │
│                                                             │
│                MOTION                                        │
│                Full animations                         on   │
└──────────────────────────────────────────────────────────────┘
```

- **Rail:** 36 px rows with an icon and label; the active row uses stronger
  text and a 2 px left accent rule, without a filled row background.
- **Header:** fixed at 56 px; 18 px semibold title, 12 px muted description,
  and a primary action on the right.
- **Content:** scrolls beneath the header, capped at a 720 px measure for
  forms, with a second column above 1000 px.
- **Grouping:** each group is a soft-fill section (`ink-850`, `10px` radius,
  no stroke) with a small caption heading, not a hard-outlined card and not a
  nested panel. Individual rows inside a section stay open (no per-row fill).
  Every section in a column shares the same width (capped at the content
  measure above) instead of shrinking to its own content — a status banner
  and the form below it line up rather than varying narrow-to-wide.
- **Scrollbar and hover/press feedback:** `accent-strong` for hover,
  `accent` for press/active, applied globally (egui shares these tokens
  between every widget and the scrollbar handle, so there is no way to tint
  the scrollbar alone without it).

Drawer mode removes the rail and uses a compact header with a section
switcher. The AI screen is the default in that mode.

#### Screen layouts

| Screen | Layout |
| :--- | :--- |
| **Identity** | Open identity header with a deterministic identicon derived from the pubkey, name, and a copyable npub chip. Content switches between Devices, Grants, Recovery, and Audit. Devices and grants are open rows with pairing state or `turn`/`pinned` scope labels; audit is a compact table with relative timestamps. |
| **Settings** | Domain list at 200 px once past 760 px, detail on the right. Appearance is the first domain because it is the one users look for. Group related controls under a heading inside a soft-fill section (`ink-850`, no border), not a whitespace-only list or a hard-outlined card. |
| **Notifications** | Filter tabs for All, Approvals, Mentions, and DMs; grouped-by-source list; 44 px rows with a source icon and a clamped title and body. Row actions appear on hover and on focus. DND toggle in the header. |
| **AI** | Two panes above 760 px: a 240 px session list and the conversation. In the drawer, a single pane with a session switcher. The header shows model and state; the context tray is a wrapping row of removable source labels above the composer; proposals use an amber leading rule and a clear approval footer rather than an outlined card. |
| **Packages** | Master and detail: a 320 px list with Installed, Updates, and Available, and a detail pane with version, size, trust badge, source, and one primary action. Use open rows and typographic hierarchy; models carry a `model` label and size estimate. |

Every list gets an empty state with one action. Every asynchronous load gets a
skeleton rather than a "Loading…" label. Every failure is an inline banner
with a retry, never a bare red line.

#### Other surfaces

- **Waybar:** floating, 8 px inset, 36 px tall. Workspaces, window title
  (muted, 40-character maximum), and system status are separated by space,
  not outlined containers. The active workspace uses a short accent underline.
  Each module is a short, readable label; icons can be added with the system
  font once that dependency is packaged.
- **Wofi:** 900×600, five columns, generous outer padding and gaps, 136×108 px
  transparent app cells, 48 px icons, and a 48 px search field. Selection is
  an underline and text accent, not a tile fill or border. Wofi has no footer,
  so key hints stay out of the launcher until the egui launcher replaces it.
- **SwayNC:** 400×680, 12 px cards with 16 px padding, priority as a 3 px
  left border, and sticky group headers.
- **Command palette:** 720×520; 48 px search field, 44 px rows grouped by
  category, and a 32 px key-hint footer.
- **Approval window:** 620×460; operation summary card, argument table,
  distinct owner and service identity rows, an accent `Approve` action, and a
  danger-outline `Deny` action.

### Motion

- Vanilla Sway cannot animate windows, round corners, blur, or cast shadows.
  The default desktop gets its polish from type, spacing, color, and
  client-side motion. SwayFX remains an optional "Glass" profile with rounded
  corners, blur, shadows, and window fades. It cannot become a hidden
  dependency.
- egui animates expand and collapse with `animate_bool_with_time` at 180 ms,
  progress and context-tray height with `animate_value_with_time`, and hover
  fills at 120 ms. Never animate window size or scroll-content height;
  animate the container instead.
- CSS surfaces use 120 ms transitions for hover, selection, and DND state.
- `swayosd` supplies the volume, brightness, and caps-lock OSD. There is
  currently no OSD at all, and its absence is one of the clearest
  "not a finished desktop" signals.
- Reduced motion is an `appearance.motion` setting that sets durations to
  zero and disables the SwayFX profile's animations.

### Theme plumbing

An `appearance` domain in `evermore-settingsd` (`scheme`, `accent`, `density`,
`font_scale`, `icon_theme`, `cursor`, `motion`) follows the existing NIP-78
model and syncs across devices. The first implementation may ship static
tokens and generated files; the follow-up adds a small `evermore-themed` user
service that watches the domain, regenerates the files, and reloads Waybar,
Wofi, SwayNC, and Sway. `ai-contextd` already exposes `settings:theme`, so AI
context is unaffected.

### Accessibility

- WCAG AA: 4.5:1 for body text and 3:1 for large text and UI boundaries.
- A visible focus ring on every interactive element; every panel flow
  reachable by keyboard; Enter activates, Escape dismisses, and arrows move
  within lists.
- Status is never color-only: pair every state with an icon or label.
- Minimum 24 px hit targets, 32 px preferred.
- Test at 1.25×, 1.5×, and 2× scale, and at the narrowest drawer width.
- egui's screen-reader support is thin. Text-heavy and accessibility-critical
  applications remain GTK or Qt, consistent with the application-set
  addendum.

### Branding assets

Logo and wordmark, four first-party app marks, the wallpaper set, a greeter
(regreet) themed from the tokens, a lock screen on `ink-950`, a Plymouth
monogram with a thin accent progress line, and a dark GRUB theme. These come
after the surfaces, not before.

### Build order

**Slice 1 — foundation and shell:**

1. `theme.toml`, `theme-gen`, the generated panel theme, and `--check`.
2. The panel `AppShell`: adaptive rail, fixed header, scroll regions, window
   sizes, and focus order.
3. Component helpers: `card`, `section`, `setting_row`, `list_row`, `badge`,
   `empty_state`, `error_banner`, `skeleton`, and `key_hint`.
4. Re-house the five existing tabs in the shell, then apply their layouts in
   order: Identity, Settings, Packages, Notifications, AI.
5. Sway client colors and gaps, the SwayNC stylesheet, the Wofi restyle, and
   the Waybar islands.

**Slice 2 — identity and polish:**

6. Bundle Inter, JetBrains Mono, and the icon font; add `.desktop` icons and
   keywords, and draw the first-party app marks.
7. Add focus rings, hover and expand motion, the reduced-motion setting,
   `swayosd`, clipboard history, and screenshot annotation.

**Slice 3 — brand and variants:**

8. Logo, wallpapers, greeter, lock screen, Plymouth, and GRUB.
9. Light and high-contrast token variants.

### Quality gates

- A `vm/ui-shots.sh` check captures the bar, Wofi, SwayNC, command palette,
  approval window, all five panel screens, and the drawer at 1× and 2× for
  review and visual regression.
- `theme-gen --check` plus a contrast assertion over the token pairs.
- A keyboard pass: every action reachable and focus always visible.
- A scale pass at 1.25×, 1.5×, and 2×, and a long-value pass for npubs, URLs,
  and package names with truncation and a copy action.
- Empty, loading, and error states reviewed for every list.

### Risks

- egui helper components need stable IDs and fixed row heights; animating
  scroll-content height is the usual source of jank.
- Font family order matters: proportional `[Inter, Noto Sans, icon]` and
  monospace `[JetBrains Mono, icon]`, so icons render inline.
- Wofi's GTK3 CSS subset cannot do shadows or hover transforms. That is the
  M10.3 trigger for the custom egui launcher, not a reason to fight the
  stylesheet.
- SwayNC's `ignore-gtk-theme: true` makes its stylesheet a standalone
  migration rather than a color tweak.
- Vanilla Sway has no rounded corners, shadows, blur, or animations, so the
  default must look finished without them.
- The panel layout is an immediate-mode retrofit: migrate one screen at a time
  behind the shell rather than rewriting all five at once.

## Addendum: Agentic AI — Bounded Agency (Design Note)

**Status:** design note, not a phase. It refines the reserved `ai-agentd` slot
in Phase 7's service table into a concrete shape and records what agentic
behaviour Evermore should ship, why, and under which limits. Phase 7's
implemented baseline is unchanged, and nothing here is a prerequisite for it.
It adds no milestones, services, or acceptance gates.

**Scope:** the assistant surface in `evermore-panel`, the inference and
operation services an agent consumes, and the audit and approval surfaces it
must use. It does not widen any existing authority, and it does not change the
build-vs-reuse decisions in the application-set addendum or the visual rules in
the design-system addendum.

### The gap this closes

Phase 7 built a complete control plane and stopped one step short of an agent.
The catalogue, signed request/approval/execution/result chain, scoped grants,
expiry, exactly-once recovery, and MCP adapter all exist. `ai-inferd` already
accepts a GBNF grammar on each chat request, which is the mechanism that makes
schema-shaped model output practical. What does not exist:

- **A model-to-operation bridge.** The panel's generic write form is typed by a
  human. Nothing turns intent into a validated, catalogue-backed proposal.
- **A task primitive.** Every unit of work is a single operation request.
  Nothing groups several requests into one reviewed piece of work.
- **A result-to-model loop.** The assistant cannot see what its own step did,
  so it cannot recover from a denial or a failed step.

Those three gaps are the whole addendum. The agentic work is therefore not an
agent framework; it is a task record, a constrained generator, and a task view
in the panel.

### Definition: bounded agency

Agentic behaviour here does not mean autonomy. It means the system proposes
work in a schema that the existing catalogue already understands, a
deterministic runtime validates and budgets it, the owner approves anything
that mutates, and every step lands in the same signed audit chain as a
hand-issued request. Four levels are worth naming separately, because
"agentic" otherwise hides which one is being discussed:

| Level | Behaviour | Status |
| :--- | :--- | :--- |
| L0 | Answers questions grounded in allowlisted local context | Implemented |
| L1 | Proposes one catalogue operation from conversation | Missing; the form exists, the generation does not |
| L2 | Proposes a plan of several operations and replans on denial or failure | Missing |
| L3 | Runs standing routines on a schedule or event under an expiring grant | Missing, and the risky one |

L1 should be done properly. L2 should be done for a small, named set of
recipes. L3 should exist only as an opt-in per routine, with its own identity
and a short grant, never as a general "the agent may act in the background"
switch.

### Why

**Product.** A local, privacy-first desktop has an agentic advantage that a
cloud assistant structurally cannot match: it can see state no remote service
can see. Which windows are open, what is installed, which trusted publishers
have signed updates, what the audit trail says happened. "Set up my writing
workspace", "why is this package broken", "what did the assistant do while I
was away" are tasks that only a local agent can do well, and they are ordinary
desktop tasks rather than AI demonstrations.

**Architecture.** Phase 7's control plane is currently exercised mainly by
tests and by the panel approving its own writes. The agent is the reason the
catalogue, the grants, and the audit trail exist as user-facing features rather
than internal plumbing. Without a consumer, the safety model has no customer
and no feedback loop.

**Positioning.** Every desktop will ship an autonomous agent. The defensible
bet is the inverse: legible, reversible, revocable agency, where the approval
surface is the product rather than an apology. That only works if there is
something worth approving every day, which is precisely what the recipes below
are for.

There is also a clear **why not**: Evermore should not ship a coding agent.
`evermore-mcp` already makes it an excellent *host* for external coding agents
over stdio or loopback HTTP. Internal agentic work aims at desktop tasks, or it
duplicates the thing the MCP boundary already does better.

### Recipes, not free-form planning

The obvious design is a ReAct loop: the model picks tools, observes results,
and iterates. That is the wrong choice here for four concrete reasons.

1. **The models are small.** The default pair is a 1B–3B base at 8–16k context.
   Free-form multi-step planning is where small models fail hardest; filling
   slots in a known template is where they are most reliable.
2. **Auditability.** A recipe is a versioned artifact with a declared
   operation allowlist, so the approval surface can show the whole shape of the
   task before the first step runs. A free-form plan can only be shown after
   the fact, which makes plan review meaningless.
3. **Testability.** Recipes can be exercised with a stub model in CI, with no
   inference at all. Free-form loops cannot be tested without a model.
4. **Prompt injection.** If untrusted text can only influence *slot values* of
   operations the recipe already declared, the blast radius is bounded. If it
   can influence tool selection, it is not.

So the rule is: **deterministic control flow in Rust, model-filled slots and
branch choices, GBNF constraints generated from the catalogue schema.** The
runtime refuses any operation outside the recipe's allowlist even when the
agent's grant permits it. Effective authority is the **intersection** of grant
and recipe, which means a compromised or manipulated model still cannot reach
an operation the recipe did not declare.

Recipes are authority-bearing artifacts and are treated like packages: shipped
in the image, or installed from a signed `npack` release with the usual
provenance. A local, hand-written recipe may live in a user directory, but it
still cannot exceed the agent's grant, and it is marked as local in the audit
record.

A recipe stays small and declarative:

```toml
id = "workspace.compose"
version = 1
title = "Compose a workspace"

operations = [
  "sway.list_windows",
  "sway.switch_workspace",
  "sway.launch_app",
  "settings.set",
  "notifications.set_dnd",
]
budget = { steps = 6, writes = 4, seconds = 90 }

[[step]]
id = "choose"
kind = "select"
schema = { workspace = "integer[1,9]", focus = "boolean" }

[[step]]
id = "focus"
operation = "sway.switch_workspace"
args = { workspace = "$choose.workspace" }
```

### The task primitive

The task is the missing abstraction, and it is most of the engineering. A task
must be persisted and resumable, because it outlives the panel window the way
an operation already outlives the CLI that submitted it.

A task record carries:

- a task ID, the recipe ID and version, the agent identity, and the owner;
- an ordered step list, where each mutating step produces a normal operation
  request and each read is an ordinary catalogue read;
- budgets: maximum steps, maximum writes, wall-clock limit, and a context
  budget, all enforced by the runtime and never by the prompt;
- lifecycle states that mirror operations plus `partially_completed` and
  `replanned`;
- a cancellation path that cancels queued steps and refuses to start the next
  one if the grant is revoked or expires mid-task.

Step requests reuse the existing signed kind:1 request event with a task tag
and a step index rather than introducing a new event kind. That keeps one
lifecycle chain, one audit view, and one recovery path, and it means approvals,
denials, expiries, and `execution_unknown` recovery already work for steps
without new machinery.

The failure path matters as much as the happy path. A denied or failed step
returns its redacted result to the model with a repair budget of two attempts;
if it still cannot proceed, the task ends `partially_completed` with the
completed steps intact. No blind retries, and no silent replanning that changes
arguments already shown to the owner. A replan is a new, reviewable step list.

### Recipes worth shipping

Ordered by value over effort, deliberately starting read-only:

| Recipe | Operations | Why it matters |
| :--- | :--- | :--- |
| **Audit digest** — what happened, what is pending, what was approved | Catalogue reads plus the local journal | Zero authority, makes the safety model legible, and is the cheapest proof of the L2 loop and the task view. |
| **Workspace composer** — "set up for writing", "focus", "meeting" | `sway.list_windows`, `sway.switch_workspace`, `sway.launch_app`, `settings.set`, and a new `notifications.set_dnd` | The visible demonstration. Reversible, keyboard-friendly, and almost entirely existing operations. The right place to prove L1 generation. |
| **Settings concierge** — "text is too small", "reduce motion", "change the accent" | `settings.get_domain`, `settings.set` | Trivial writes, and the natural test bed for plan preview and approval fatigue. |
| **Package doctor** — "why is this broken", "what can I update" | `packages.list_installed`, `packages.list_available`, `packages.explain_dependencies`, then `install_pinned` or `remove` as a proposal | The strongest agent-as-administrator story without a shell, and it exercises `npack`'s signed-publisher trust model directly. Read-heavy, at most one write. |
| **Notification triage** — group, summarise, propose dismissals | `notifications.list_recent` for non-DM records, then `dismiss` per item | High daily value. Batch dismissal is N requests under one task, not a new batch operation. |
| **Update planner** — plan updates from trusted publishers | `packages.list_available`, `packages.explain_dependencies`, then `install_pinned` per package | Genuine multi-step value, and partial failure is the expected outcome rather than an error. |

`notifications.set_dnd` is the only new operation the first set needs; SwayNC
already exposes the toggle. Everything else is existing catalogue surface,
which is the point: the agent's first year should consume the catalogue rather
than expand it.

Deliberately deferred: anything requiring filesystem or notes writes. The
catalogue has no raw file operation and should not gain one. If Notes becomes a
real application, add `notes.read` and `notes.write` scoped to that store, not
a general file API. Voice input, scheduling beyond per-routine opt-in, and any
"do anything" assistant mode are also out of scope.

### Authority and identities

- The agent is the reserved `ai-agentd` slot, implemented as a deterministic
  recipe runner: a small state machine that owns inference calls, catalogue
  reads, and request submission. It never holds execution authority; that
  stays with `evermore-operationsd`.
- v1 ships one interactive **Evermore Assistant** identity with a narrow,
  exact-operation grant. Standing routines get their own identity so that a
  long-lived grant is visibly distinct in the audit trail. Multiple identities
  per capability would multiply approval fatigue for no security gain.
- The agent may request a grant. It may never create one, extend one, set its
  own deadline, choose its own actor, or approve its own request. Those
  restrictions are already enforced in `evermore-operationsd` and are
  restated here because a planner is exactly the component that would be
  tempted to work around them.
- A recipe's operation allowlist and the agent's grant are both checked at
  submission, at approval, and at execution, so revoking a grant mid-task stops
  the task before its next write.
- Reads execute without prompting under the grant; writes always require an
  owner decision.

### What it must never do

- Execute a shell, spawn an arbitrary process, or open an arbitrary network
  connection. `sway.launch_app` keeps its bounded argv and direct `exec`.
- Perform a write without an approval event, including inside an approved plan.
  Plan approval pre-approves a specific canonical list of arguments; it does
  not create a session-wide permission.
- Read direct messages or any context source outside the user-session
  allowlist.
- Loop indefinitely, retry a denied step, or run a background routine without a
  per-routine expiring grant.
- Modify its own recipes, budgets, or allowlists at runtime.
- Present a model's stated reason as a system fact. The approval surface shows
  the model's reason *and* the triggering content, labelled as such.

### Prompt injection

An agent with local sensors and write proposals is the highest-value injection
target in the system: a notification body, a package description, a note, or an
indexed file can all contain instructions aimed at the model. The defences are
structural rather than prompt-level.

- Untrusted content is tagged as data in the prompt and never placed in the
  instruction region.
- Tool selection is fixed by the recipe, so injected text can only influence
  declared slots, and every slot is schema-validated by the catalogue before
  submission.
- Every write still requires owner approval, and the approval card displays the
  triggering snippet next to the proposed arguments, so a steered proposal is
  visible at the moment of decision.
- Results returned to the model are redacted first, using the existing
  `Redaction` policy, so a step cannot leak a secret into the next turn.
- The agent's identity and grant come from configuration and the owner, never
  from content the model has seen.

### Interface

The panel gains a **Tasks** view beside the pending-request inbox and the
activity history. It shows the recipe, a step timeline with per-step state, and
the plan preview before anything runs. Approvals present all pending writes of
a task together with their exact canonical arguments, one primary approve
action, and per-step deny. A finished task reads as a short narrative with a
result summary, not as a JSON dump.

This is where the visual design system's components pay off: task cards, step
badges, and a detail pane reuse the same `card`, `badge`, `list_row`, and
`empty_state` helpers as Settings and Notifications. Long-running tasks report
through the existing notification path. The command palette can route an
imperative phrase such as "make text bigger" into the matching recipe, falling
back to chat when no recipe matches.

The AI drawer keeps the conversational surface and adds a single line of task
status when one is running, so the drawer does not need a second layout.

### Implementation order

Layout and the task view come first, consistent with the design-system
addendum's layout-first rule.

1. **Task primitive and audit digest.** Task records, the task tag on request
   events, the panel task view, and a read-only digest recipe. No new
   authority, no new operation.
2. **L1 generation.** Constrained JSON from `ai-inferd`, validated against the
   catalogue, with the two-attempt repair loop. Replace the manual proposal
   form with "describe what you want" for a single operation.
3. **Workspace composer and settings concierge.** First writes, the plan
   preview and plan approval, and `notifications.set_dnd`.
4. **Package doctor and update planner.** Read-heavy multi-step work with
   `partially_completed` as a normal outcome.
5. **Routines.** Scheduled recipes, dedicated identities, short grants, and
   audit visibility. Opt-in only.

Quality bars for each step: recipes tested with a stub model; the repair loop
tested against malformed and out-of-allowlist output; denial, expiry, grant
revocation mid-task, and daemon restart covered; and a task-level screenshot in
`vm/ui-shots.sh`.

### Open decisions

- **Recipe-first, with free-form restricted to reads.** This is the
  recommendation above. The alternative, letting the model choose tools
  freely and treating recipes as hints, restores the observability and
  injection problems the catalogue exists to solve.
- **Plan-level approval.** One approval covering a canonical list of writes is
  better UX than three interruptions, but it is a real security trade-off.
  Per-step approval is the conservative default and should remain available.
- **Whether L2 is justified on a 1B–3B model.** It may be that the first
  release ships L1 plus deterministic recipes with slot extraction only, and
  defers replanning until a larger default model or the consent-gated cloud
  path exists. That changes the recipe schema, so it should be settled before
  the schema is frozen.
- **Identity granularity.** One interactive identity plus one per routine is
  the current lean. Per-capability identities would make grants more legible
  and approvals more frequent.

### Risks

- Small models hallucinate arguments confidently. The catalogue validator, not
  the prompt, must be the gate, and the repair loop must be bounded.
- Task grouping via tags must not become a second lifecycle. Steps use the
  existing request events and states, so recovery, expiry, and
  `execution_unknown` behaviour stay in one place.
- Plan approval can quietly become blanket permission if arguments are not
  canonicalised and revalidated at execution. It must bind exactly as a single
  approval does.
- Recipes are authority-bearing. An unsigned local recipe that can name writes
  is a privilege-escalation path unless it is capped by the grant and marked in
  the audit record.
- The approval surface is the product. If a task requires more than one or two
  decisions, users will start approving without reading, which erases the
  safety benefit faster than any bug.
- Conversations and task state are new local data with their own retention
  question. They belong in the private user store, subject to the same export
  and deletion rules as the operation journal.

## Addendum: Nostr Calendar Integration — Design Note

**Status:** public and private event vertical slice implemented; broader NIP-52
operations and application interoperability remain staged work.

**Goal:** make NIP-52 calendars useful to the owner, local applications, and
Evermore AI through the existing operation control plane. Calendar records
are ordinary NIP-52 events on explicitly chosen external relays. The local
control-plane relay stores a local copy for offline reads and applications;
this does not publish an event to an external relay unless the owner has
selected that destination.

### Protocol mapping

Reuse [NIP-52](https://github.com/nostr-protocol/nips/blob/master/52.md):

| Kind | NIP-52 meaning | Evermore use |
| :--- | :--- | :--- |
| `31922` | Date-based calendar event | All-day and multi-day entries; `end` is exclusive |
| `31923` | Time-based calendar event | Timed entries using Unix seconds, optional IANA TZID, and `D` day-index tags |
| `31924` | Calendar addressable list | Named collections such as Personal, Work, and Shared; its repeated `a` tags point to event coordinates |
| `31925` | RSVP addressable event | The attendee's `accepted`, `declined`, or `tentative` status, optionally with `fb=free/busy` |

Treat NIP-52 as a draft interoperability format, and preserve unknown tags and
content when an event is edited. Events may request inclusion in a calendar
with an `a` tag; that request is not authorization. The calendar owner decides
whether to include it. Do not imply that an RSVP grants access or settles
attendance policy. NIP-52 intentionally has no recurrence model, and these
event kinds are not private by default. Public NIP-52 events must never be
presented as private merely because their `.content` is encrypted; the title,
times, location, participants, and other tags remain visible. Recurring
schedules and free/busy disclosure need explicit product decisions before
support.

### Private events: NIP-59 gift-wrapped NIP-52 rumors

For private events, use NIP-59 gift wrapping rather than publishing the NIP-52
event with only its description encrypted. Put the full NIP-52-shaped event
(kind `31922` or `31923`, including sensitive tags and content) in an unsigned
rumor, seal it as kind `13` with the owner's signer, then create a separate
kind `1059` gift wrap for each invited recipient. The outer `p` tag lets relays
route the wrap; the event fields and real author are revealed only after the
recipient unwraps it. Private RSVP kind `31925` can use the same recipient
delivery pattern. Use durable kind `1059` for calendar invitations and updates,
not ephemeral kind `21059`.

This is a NIP-59 application convention, not a standard NIP-52 private-event
profile: NIP-59 defines wrapping any event but leaves concrete application
protocols to other specifications. Therefore only Evermore-aware clients can
discover, decrypt, replace, and display these wrapped calendar records; normal
NIP-52 clients and public calendar indexes will not see them. The outer relay
still learns each recipient pubkey, publication timing and volume, and relay
connection metadata. NIP-59 reduces exposed event metadata; it does not provide
anonymity.

For a first version, recipients explicitly listed by the owner are the access
list, with one wrap per recipient and a self-addressed wrap for the owner's
other devices. Unwrap and verify the kind-13 seal before trusting the inner
author or tags. Locally deduplicate and resolve revisions using the inner
`kind:pubkey:d` coordinate and signed seal provenance; relay addressable-event
replacement does not apply to the outer kind-1059 wraps. Private edits send a
new rumor to the existing recipient set. Private deletion sends an encrypted
kind-5 withdrawal rumor to each recipient, and Evermore-aware clients hide the
matching event locally. This cannot remove copies already retained by a
recipient, and does not claim private-calendar interoperability with other
Nostr clients. Changing an event's recipient set remains a separate invite
and revocation flow.

### Service and data flow

Keep signed calendar events in the existing local control-plane relay, which
already provides durable storage, event verification, subscriptions, and
NIP-86 kind policy. Add NIP-52 kinds `31922`–`31925` to its allowlist. The
control-plane relay is the local offline cache; configured external relays
carry the user's cross-device events. Do not keep a second JSON event store.
Initially, `evermore-operationsd` can own calendar queries, projection, and a
small background sync loop alongside its existing operation recovery loop.
It uses `evermore-signer` for signatures and never receives or persists an
`nsec`. Extract a separate calendar sync service only if its lifecycle or
failure isolation becomes a demonstrated need.

Use these boundaries:

1. Configured external relays carry public NIP-52 events and kind-1059 wraps
   addressed to the owner or invited recipients. Sync is scoped to
   owner-selected relays; the local control-plane relay stores verified events
   for offline reads and local applications.
2. `evermore-operationsd` exposes the normalized calendar query surface via
   its existing Unix socket. Panel, CLI, MCP, and other local clients consume
   the same `calendar.list`/`calendar.get` operations rather than each
   implementing relay queries and NIP-52 parsing.
3. `evermore-operationsd` remains the only AI/application path for actions
   requiring authority. Add catalogue reads such as `calendar.list`,
   `calendar.get`, and `calendar.find_availability`, plus approval-required
   writes such as `calendar.create_event`, `calendar.update_event`,
   `calendar.delete_event`, `calendar.respond_rsvp`, and
   `calendar.include_event`. Define grant names and input/output schemas in
   `evermore-common`; MCP then exposes the same definitions automatically.
4. Writes become signed requests bound to canonical event fields and target
   calendar coordinates and visibility. The owner approval is recorded in the
   existing operation lifecycle; only after approval does operationsd publish
   the public NIP-52 event to the selected remote relay and local relay, or
   construct recipient gift wraps. Public deletions use NIP-09 deletion events
   and require the same explicit review. Private deletion and update delivery
   must follow the private-event sync rules above. Retries are idempotent by
   operation ID and event coordinate.

The local relay is the local canonical event cache, while selected external
relays provide cross-device distribution. Do not invent Evermore event kinds
for events, calendars, or RSVPs.

### AI and application use

AI gets read access only through catalogue tools, with clear time-zone and
freshness metadata. A first calendar recipe can answer “what is next?”, find a
free slot across selected calendars, or summarize the day's agenda. Availability
must consider accepted/tentative RSVP policy explicitly and must never infer
that an unselected calendar is empty. Model context excludes participant
details and event descriptions unless the user has selected that scope.

For creation or edits, AI produces a preview with title, local date/time,
time-zone, calendar, participants, visibility/publication destination, and
exact operation. The review surface reuses Phase 7's amber-leading-rule
proposal and clear approval footer. No hard-outlined card stack: use the
Graphite open-row and soft-fill grouping language. Reject ambiguous time zones
or missing dates rather than letting the model guess. Each mutation requires
the normal owner approval; read grants do not imply write permission.

The panel can provide an agenda screen and event detail/actions as a control
surface, while a conversational “what's next?” lives in the existing AI
drawer. Do not build a standalone calendar application. Begin application
interoperability with an explicit `.ics` import/export path. Add a CalDAV or
Evolution Data Server bridge only after a target application is selected and
its sync, conflict, and credential model is validated; NIP-52 itself is not a
drop-in CalDAV backend. Keep the local socket/API stable so future bridges do
not change the AI and panel contracts.

### Suggested delivery order

**Implementation status (2026-11-08):** the calendar event and mutation vertical slice is implemented.
The local control-plane relay accepts NIP-52 kinds `31922`–`31925`;
`calendar.list` and `calendar.get` read and normalize events from that existing
store, and MCP consumes the same catalogue. A background operationsd worker
pulls public events, deletions, and recipient-addressed private wraps from
configured external relays into the local relay. AI access is an explicit `calendar:events` context source, off by
default, with the panel's existing one-turn/pinned scope controls. The
catalogue includes owner-approved calendar create, update, and delete
operations. Operationsd publishes public kind-31923 events and NIP-09 deletion
requests, or seals and gift-wraps private kind-31923 rumors and private
withdrawal rumors for each participant and the owner. Edits retain the same
addressable coordinate and private recipient set. Private wraps sync from
configured relays and are unwrapped and verified through the owner signer
before appearing in local reads; private withdrawals hide matching events in
Evermore-aware clients. Date-based event creation, RSVP and
calendar-list writes, agenda UI, and app bridges remain to be implemented.

1. Validate NIP-52 parsing/serialization and preservation against fixture
events for all four kinds; handle date-only versus timestamp semantics,
exclusive end dates, TZIDs, `D` tags, and address coordinates.
2. Validate configured-relay public and private sync into the local relay;
   verify offline reads and NIP-09 replacement/deletion handling.
3. Expose catalogue reads to the panel and MCP; test AI agenda and availability
   with a deterministic fixture set, including overlapping events and DST
   boundaries.
4. Add public RSVP as a separately granted, owner-approved operation and
   validate signed relay publication, retries, denial, and restart recovery in
   the VM.
5. Fixture-test private kind-1059 updates, duplicate delivery, withdrawal
   behavior, and recipient visibility; then add approved private RSVP.
6. Add incoming calendar inclusion requests and conflict handling after
   destructive-review wording and NIP-09 behavior are validated in the VM.
7. Evaluate one real calendar application for `.ics` or a standards bridge;
   implement only the adapter that proves useful.

The NostrHost lesson applies directly: ship one complete, recoverable journey
before accumulating integrations. First prove configured relay → local
projection → AI read, then an approved write → signed NIP-52 publication →
local refresh. Keep owner, AI actor, and service signer distinct throughout,
and make network or relay failure visible without losing the locally cached
agenda or replaying a write.

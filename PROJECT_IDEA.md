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
    evermore-mcp/               <- Phase 7: MCP server exposing daemons to AI agents
  apps/
    evermore-panel/             <- egui control panel (tabs: identity, settings, notifications, packages)
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

**Milestones:**
- **M4.1** — `evermore-blossom-fs` implements the Blossom HTTP API client (upload/get/list/mirror) authorized via `evermore-signer`'s socket (NIP-98-style signed auth events, not a stored API key).
- **M4.2** — First surface: a **sync daemon** for `~/Nostr/` (simpler than FUSE to start) using `notify`/inotify — new/changed files upload to Blossom and get indexed in a `kind:30000`-ish addressable "file index" event per directory, signed by the user's key.
- **M4.3** — FUSE mount (`fuser` crate) as the second, richer surface once M4.2's upload/index logic is proven — mounts a Blossom server read/write at a configurable path, so it shows up as an ordinary filesystem location (a GVFS backend is the GNOME-specific equivalent, evaluated only if FUSE proves awkward for Nautilus/Files integration).
- **M4.4** — File-manager integration: a "Publish to Nostr" Nautilus/Files extension (or a right-click script as a stopgap) that uploads the selected file via M4.1 and copies a `nostr:`/NIP-92 `imeta` reference to the clipboard.

**Validation:** Drop a file into `~/Nostr/` on one VM; from a second VM/identity pairing to the same key, confirm the file is retrievable (via the FUSE mount or a direct Blossom fetch) without manually copying anything.

### Phase 5: `npack` as the System Package Manager

**Goal:** A bootable Arch-based ISO where `npack` is the primary software distribution mechanism, bootstrapped with a minimal default trust set.

**Milestones:**
- **M5.1** — `evermore-npack` core: `.npk` archive format (likely a signed tarball — reuse an existing signed-archive crate rather than inventing one), publisher trust represented as **NIP-51 lists** on the local control-plane relay (`evermore:trusted-publishers`), not a bespoke trust-graph file.
- **M5.2** — Package discovery via NIP-89 handlers + `kind:32267` (software-application) and `kind:30063` (release-artifact sets) events, fetched from the publishers in the user's trust list — no central index to query.
- **M5.3** — Default trust seed: a hardcoded bootstrap list (the distribution's own publisher key + a handful of foundational packages) written into `evermore:trusted-publishers` on first boot, editable afterward — ships *as data*, not as a separate mechanism.
- **M5.4** — Install/update/remove flow: resolve → fetch via Blossom (Phase 4's client) → verify signature against the trust list → unpack → register with systemd/the filesystem.
- **M5.5** — `apps/evermore-panel`'s Packages tab: browse discovered packages (grouped by trusted publisher and, later, social follows), install/update/remove actions, and a "Software Sources" view that's really just `evermore-settingsd`'s `evermore:trusted-publishers` and `evermore:relays` domains surfaced as UI.
- **M5.6** — `packaging/archiso/` profile based on `archiso`'s `baseline`: kernel, a Wayland compositor (plain Sway — see "Key Design Decisions" on not building the shell in egui), the pinned Rust toolchain, and all Phase 0-5 daemons pre-installed and enabled.

**Validation:** Build the ISO (`mkarchiso`), boot it in a *fresh* VM (not the dev build VM — a clean target, built the same way `vm/create-vm.sh` builds the dev one), and install one new egui application end-to-end through `npack` using only the default trust set and the ISO's preconfigured relays.

### Phase 6: Dogfood & Harden

**Goal:** Stop treating this as a demo and start running it as a daily system on at least one real machine, finding the rough edges Phases 0-5's narrow validations didn't surface.

**Milestones:**
- **M6.1** — Run the ISO (or the equivalent package set on an existing Arch install) as a real daily session for a defined period; log every manual workaround needed as a tracked issue.
- **M6.2** — Security pass on the two trust boundaries that matter most: the signing daemon's socket permissions/identity-switching, and the local relay's NIP-86 policy (can anything on the loopback plane write kinds it shouldn't?). Written up the same way `imattau/nostrhost`'s `SECURITY-REVIEW.md` does it.
- **M6.3** — Cross-device validation for real: two *different physical machines* (not two VMs on one host) signed in with the same key, confirming settings/notifications/files/packages all actually follow the identity as designed, not just within one host's loopback relay.
- **M6.4** — Backup/recovery story for the `nsec`: NIP-49 `ncryptsec` export/import flow through the signing daemon, tested by deliberately wiping a VM's keyring and recovering from the exported backup.
- **M6.5** — Decide and document the public release shape (ISO download, AUR packages for the daemons on stock Arch, or both) based on what Phase 5/6 actually proved out.

### Phase 7: AI & Agentic AI Integration

**Goal:** Let AI agents — a local assistant, an autonomous coding agent, any
MCP-connected tool — act through Evermore's existing daemons safely, with
the same scoped-permission and audit-trail discipline `imattau/nostrhost`
already established for its own agent/owner split, rather than each agent
integration inventing its own bespoke access path. This builds entirely on
Phase 1's signer and Phase 2's relay; it adds a grant/scope layer on top,
not a parallel system.

**Milestones:**
- **M7.1** — Agent identities are separate Nostr keypairs, never the owner's
  `nsec`. A grant (which kinds, which settings domains an agent may touch)
  is itself an addressable control-plane relay event
  (`evermore:agent-grant:<agent-pubkey>`), signed by the owner identity —
  mirrors nostrhost's scope-tier model: no agent identity ever holds
  "administrator," because owner co-signature already covers the risk that
  tier existed to gate.
- **M7.2** — `evermore-signer` enforces grants at the socket boundary: an
  agent's `sign_event`/`nip44_encrypt` requests are checked against its
  grant *before* anything is signed, and anything out of scope is refused
  outright, not merely logged. A delegated/agent identity is explicitly
  barred from approving its own privileged request — approval of anything
  beyond the base grant requires a separate event signed by the *owner*
  identity (the self-approval delegation guard pattern) — closing exactly
  the agent-self-approval path nostrhost had to close after the fact.
- **M7.3** — `evermore-mcp`: an MCP server (Rust, reusing the same
  Unix-socket clients already written for `evermore-panel`) exposing
  read-mostly tools over Evermore's daemons to AI coding assistants and
  agent runtimes — list identities (never secrets), read settings, list
  recent notifications, query `npack`'s trust graph — plus one scoped
  "propose an event" tool that routes through M7.2's grant check rather
  than a raw signing passthrough.
- **M7.4** — Every agent-initiated action rides the same request → approval
  → execution → result audit chain nostrhost's `CONTROL-PLANE.md` already
  established for Evermore's control-plane relay (regular, immutable
  events) — an agent's actions get the same cryptographic provenance as a
  human's, not a separate, unaudited path.
- **M7.5** — `npack`: an agent may *propose* a package update via M7.3's
  tool, but publishing still requires the publisher identity's own
  signature — an agent can draft a package, never ship one.
- **M7.6** *(optional, later)* — a chat-style Assistant tab in
  `evermore-panel` that talks to a local model, whose own tool access is
  itself just another agent grant under M7.1 — the panel gets no special
  bypass of the scope/approval machinery everything else goes through.

**Validation:** Issue a scoped grant to a throwaway agent identity (e.g.
"may publish `kind:1` under `evermore:test` only"); confirm `evermore-signer`
signs requests within that scope, refuses an out-of-scope request, and
refuses an attempted self-approval; confirm the resulting events carry the
same request/approval/execution/result audit-chain shape as Phase 2's
human-initiated settings changes — an agent's footprint should be
indistinguishable in form from a human's, just distinguishable in *who
signed what*.

## Key Design Decisions and Risks

A few decisions will shape the entire project and are worth resolving early:

- **Trust bootstrapping for `npack`.** The system needs a usable default trust set on first boot, or the package manager is a blank slate. Decide whether the ISO ships with a curated publisher list, a "trust on first use" model, or a hybrid. The `npack` project's explicit "no central repository" design means *you* must provide the initial social graph seed.
- **Key storage strategy.** The kernel keyring is excellent for session-scoped keys and is already used by the signing daemon pattern. For at-rest backup and cross-device portability, NIP-49 `ncryptsec` is the right format. The system secret service (GNOME Keyring / KWallet) is a viable alternative for desktop-integrated storage. Pick one primary path and make the signing daemon the only component that ever sees the decrypted key.
- **`smithay-egui` is not a shell.** It is explicitly "mainly intended for debug interfaces". Do **not** plan to build the entire desktop shell in egui. Use egui for the *applications and control panels* (settings, package manager, notification center) and rely on a conventional Wayland compositor (e.g., Sway, or a Smithay-based compositor that is not trying to be a full shell) for window management. This keeps the risk bounded.
- **D-Bus vs. Unix socket for the signing interface.** A Unix socket is simpler and more portable across languages; D-Bus gives you standard service discovery and permission integration. The NanoClaw pattern uses a Unix socket with a read-only mount into containers, which is a clean security boundary. Start with a Unix socket and add a D-Bus wrapper later if integration with existing desktop services demands it.
- **NIP-78 encryption and relay auth.** NIP-78 settings should be encrypted with NIP-44 and relays should require NIP-42 `AUTH` before serving `kind:30078` events. This is not optional for a production system—without it, settings are either public or served to the wrong clients.
- **Agent identities are never the owner's key.** Any AI/agentic integration (a local assistant, an MCP-connected coding agent, an autonomous process) gets its own Nostr keypair and an explicit, scoped grant — never delegated access to the owner's real `nsec`, and never permitted to approve its own privileged requests. This is the same split `imattau/nostrhost` already uses for its agent/owner model (see Phase 7); Evermore reuses it rather than re-deriving a weaker version from scratch.

The first concrete step is **M0.1**: scaffold the workspace. The first step that actually matters is **Phase 1**: get the signing daemon running on Arch and expose it to a trivial egui application. Once that interface is stable, every subsequent service—settings, notifications, Blossom, `npack`—can be built against it without re-solving key management. This is the smallest, most reusable foundation for the entire distribution.

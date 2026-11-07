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

The phases are ordered by dependency and risk. Each phase delivers a working, testable slice of the integrated system before moving on.

### Phase 1: The Signing Foundation (Identity as a Service)

**Goal:** A working Nostr signing daemon on a standard Arch install, exposing a stable interface that any application (including egui apps) can use without ever touching the raw `nsec`.

**Concrete work:**
- Build or adapt a **signing daemon** that stores the `nsec` in the **Linux kernel keyring** and exposes a Unix socket (`$XDG_RUNTIME_DIR/nostr-signer.sock`) with methods like `sign_event`, `get_public_key`, `nip44_encrypt`, `nip44_decrypt`, and `wrap_dm`. This pattern is already production-proven in the NanoClaw project.
- Package it as a **systemd user service** that starts with the session and holds multiple identities (personal, work) selectable by account name.
- Write a minimal **egui control panel** that lets the user unlock the daemon, list loaded identities, and view their public keys—your first integration UI.

**Validation:** An egui test application signs a note and uploads a file to Blossom through the daemon socket, without the `nsec` ever entering the application process.

### Phase 2: Settings That Follow the Key

**Goal:** Desktop preferences that sync across machines via NIP-78, so "sign in once and your settings follow you" becomes real.

**Concrete work:**
- Stand up the **local control-plane relay** (khatru, localhost-only, from
  "The Local Control-Plane Relay" above) as a systemd user service — this is
  the first thing the settings daemon talks to, and later phases (Phase 3's
  notification bridge, Phase 5's `npack` trust layer) reuse the same relay.
- Build a **settings daemon** (systemd user service) that owns the NIP-78 sync loop. It reads local config changes and publishes encrypted `kind:30078` events with a `d` tag identifying the application or settings domain, to the local relay, which selectively bridges them to the user's external relays.
- Define a minimal **desktop settings schema**: theme, keyboard layout, panel configuration, and—critically—**relay configuration** (which relays the system uses for settings, DMs, and package discovery).
- Expose a **D-Bus interface** (or a local HTTP socket) so applications and system components can read and write settings through the daemon rather than managing their own config files.
- Build an **egui Settings panel** that reads from and writes to this daemon, showing current relay choices and sync status.

**Validation:** Change a setting on one machine (e.g., theme or relay list), and it appears on a second machine logged in with the same Nostr key.

### Phase 3: Nostr-Aware Notifications

**Goal:** Mentions, replies, reactions, zaps, and DMs arrive as native desktop notifications, indistinguishable in form from local application alerts.

**Concrete work:**
- Build a **notification bridge daemon** that subscribes to the user's Nostr streams for the relevant event kinds (mentions, replies, DMs via NIP-17 gift wrap) — both external relays and the local control-plane relay from Phase 2, so system-local events (e.g. a finished build, a package update available) arrive through the same path as social notifications. When an event arrives, it calls the standard `org.freedesktop.Notifications` D-Bus interface to show a desktop toast.
- Optionally, implement the **reverse direction**: a systemd user service that monitors local D-Bus notifications and forwards them into a Nostr chat stream (the pattern used in `spaces-os`).
- Build an **egui notification center** that displays a merged timeline of local and Nostr notifications, with filters by source.

**Validation:** A mention on Nostr produces a desktop notification while a local build finishes, and both appear in the same notification center.

### Phase 4: Blossom as a Filesystem Surface

**Goal:** Make Blossom storage a first-class part of the file manager without replacing the local filesystem.

**Concrete work:**
- Implement a **Blossom FUSE mount** (or a GVFS backend) that authenticates with the signing daemon and presents a Blossom server as a network location in the file manager (e.g., a "Nostr Files" sidebar entry). Garland already demonstrates this "virtual drive" model.
- Alternatively, build a **sync daemon** for a designated folder (`~/Nostr`) that watches for changes, uploads new files to Blossom, and publishes signed file-index events.
- Add a **"Publish to Nostr" context-menu action** in the file manager that uploads a file to Blossom and copies a `nostr:` or NIP-92 `imeta` reference.

**Validation:** A user copies a file into the Blossom mount, and it is retrievable from another machine via the same Nostr key.

### Phase 5: `npack` as the System Package Manager

**Goal:** A bootable Arch-based ISO where `npack` is the primary software distribution mechanism, bootstrapped with a minimal default trust set.

**Concrete work:**
- Use **`archiso`** with the `baseline` profile to build a minimal ISO, including the kernel, Wayland compositor, Rust toolchain, and—critically—`npack` itself as a pre-installed system package.
- Ship a **minimal default trust set** (the distribution's publisher key plus a few foundational packages) because a blank trust graph is not usable. Represent it as NIP-51 lists on the local control-plane relay (from Phase 2), so trust edits are just events, and make the set editable in the Settings panel.
- Build an **egui package manager frontend** that wraps `npack`, showing packages discovered through the user's trust graph and social follows, with install/update/remove actions.
- Integrate **"Software Sources" into the Settings daemon**: a panel for managing trusted publishers, relays, and Blossom servers (the `npack` equivalent of repository configuration).

**Validation:** A fresh ISO boots, and a user can install a new egui application entirely through the Nostr-native `npack` loop, using only their configured relays and trust set.

## Key Design Decisions and Risks

A few decisions will shape the entire project and are worth resolving early:

- **Trust bootstrapping for `npack`.** The system needs a usable default trust set on first boot, or the package manager is a blank slate. Decide whether the ISO ships with a curated publisher list, a "trust on first use" model, or a hybrid. The `npack` project's explicit "no central repository" design means *you* must provide the initial social graph seed.
- **Key storage strategy.** The kernel keyring is excellent for session-scoped keys and is already used by the signing daemon pattern. For at-rest backup and cross-device portability, NIP-49 `ncryptsec` is the right format. The system secret service (GNOME Keyring / KWallet) is a viable alternative for desktop-integrated storage. Pick one primary path and make the signing daemon the only component that ever sees the decrypted key.
- **`smithay-egui` is not a shell.** It is explicitly "mainly intended for debug interfaces". Do **not** plan to build the entire desktop shell in egui. Use egui for the *applications and control panels* (settings, package manager, notification center) and rely on a conventional Wayland compositor (e.g., Sway, or a Smithay-based compositor that is not trying to be a full shell) for window management. This keeps the risk bounded.
- **D-Bus vs. Unix socket for the signing interface.** A Unix socket is simpler and more portable across languages; D-Bus gives you standard service discovery and permission integration. The NanoClaw pattern uses a Unix socket with a read-only mount into containers, which is a clean security boundary. Start with a Unix socket and add a D-Bus wrapper later if integration with existing desktop services demands it.
- **NIP-78 encryption and relay auth.** NIP-78 settings should be encrypted with NIP-44 and relays should require NIP-42 `AUTH` before serving `kind:30078` events. This is not optional for a production system—without it, settings are either public or served to the wrong clients.

The first concrete step is **Phase 1**: get the signing daemon running on Arch and expose it to a trivial egui application. Once that interface is stable, every subsequent service—settings, notifications, Blossom, `npack`—can be built against it without re-solving key management. This is the smallest, most reusable foundation for the entire distribution.

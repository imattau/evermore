# Evermore basics implementation roadmap

This roadmap turns the basics checklist into readiness-gated work for the
Arch-based Sway/Waybar prototype. Existing features remain experimental until
their phase gate has evidence in `application-readiness.md`.

## Current implementation progress

- **Phase 1:** appearance controls, synced save feedback, and reset are
  implemented in the panel. The full readiness gate has not been verified.
- **Phase 2:** external relay management/testing, notification source controls,
  and local-time quiet-hours DND are implemented. Identity/device management,
  live relay subscription refresh, and the phase acceptance gate remain open.
- **Phases 3–6:** planned; implementation and acceptance evidence remain open.

## Phase 1 — Settings foundation and appearance

**Deliver:** organized Settings sections; saved-state, retry, reset, and
local-versus-synced affordances; theme, accent, density, text scale, and
reduced-motion preferences. Shareable appearance preferences sync through the
existing settings service. Keep hardware and desktop-layout preferences local.

**Gate:** values persist after restart; unavailable settings service shows a
clear retry path; reset restores defaults; all controls work by keyboard with
visible focus.

## Phase 2 — Connectivity, notifications, and identity

**Deliver:** relay list/add/remove/reorder/test and connection state; local
network controls through the desktop environment; notification source controls,
quiet hours, and DND schedule; active identity, signer/lock, device, and sync
status with deliberate device revocation. Keep per-device network choices
local.

**Gate:** exercise online/offline, unavailable relay, locked signer, stale
device, and notification service restart. Sensitive identity actions are
reviewable and attributable.

## Phase 3 — AI privacy and software trust

**Deliver:** visible AI source scope, `.no-ai` exclusions, redaction, retention,
model identity and local-inference state; software/model update status and
publisher trust through the existing `npack-gui` flow.

**Gate:** excluded content never enters a request; unavailable inference,
corrupt/untrusted packages, interrupted operations, and trust changes have
clear outcomes and recovery.

## Phase 4 — First run and everyday desktop

**Deliver:** onboarding for identity, optional phone pairing, network, and sync
expectations; entry points for Settings, software, Help, files, terminal, and
Firefox; familiar upstream controls for network, audio, display, Bluetooth,
power, and screen lock; local launcher/startup/workspace preferences. Reuse
Nautilus, Foot, Firefox, XDG desktop entries, and established system tools.

**Gate:** a clean profile completes setup, logs in, finds core apps and system
controls, and recovers from interrupted enrollment without affecting existing
accounts.

## Phase 5 — Sync, recovery, help, and accessibility

**Deliver:** Blossom storage/sync state, upload failures and conflict recovery;
searchable troubleshooting; user-triggered diagnostic export that excludes
secrets; keyboard, focus, scaling, reduced-motion, and supported screen-reader
improvements. Prepare strings for translation; English remains the initial
language.

**Gate:** two-device sync and conflict scenarios preserve the only local copy;
diagnostic exports contain no secrets; accessibility checks cover keyboard
operation and supported scale settings.

## Phase 6 — Whole-desktop acceptance

**Deliver:** run the daily-use path on a clean install and documented reference
device; repeat after reboot and with network, relay, signer, and inference
unavailable; exercise interrupted enrollment/software operations, service
restarts, and upgrade state preservation. Publish supported/experimental/
unavailable status and exact environment assumptions.

**Exit gate:** each in-scope flow has observable success, understandable
failure behavior, and tested recovery. Do not promote an unaccepted feature to
supported status.

## Defaults and boundaries

- The supported desktop target remains the repository's Arch-based
  Sway/Waybar setup.
- Shareable Evermore appearance preferences sync by identity. Hardware,
  network, desktop layout, startup applications, and shortcuts are local.
- Firefox is the default browser; users retain normal XDG default-app choice.
- English is the initial language. Translation follows localization
  preparation.
- Use upstream desktop applications for browser, files, terminal, and hardware
  controls. Evermore owns its identity, trust, approvals, AI context, settings,
  notifications, and service status.


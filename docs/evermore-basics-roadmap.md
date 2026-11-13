# Evermore basics implementation roadmap

This roadmap turns the basics checklist into readiness-gated work for the
Arch-based Sway/Waybar prototype. Existing features remain experimental until
their phase gate has evidence in `application-readiness.md`.

## Current implementation progress

- **Phase 1:** appearance controls, synced save feedback, and reset are
  implemented in the panel. The full readiness gate has not been verified.
- **Phase 2:** external relay management/testing, notification source controls,
  local-time quiet-hours DND, and a public-profile name editor using NIP-01
  kind 0 events are implemented. Device management, live relay subscription
  refresh, and the phase acceptance gate remain open.
- **Phase 3:** AI source scopes, `.no-ai`, redaction, local model selection, and
  trusted-publisher visibility and confirmed trust/revoke actions are present.
  A local conversation-retention choice now syncs by identity and prunes only
  this device's expired history. Settings checks package/model updates
  through the local operations service, reports available versions, revoked
  releases, and resolver failures, and leaves installation behind its existing
  approval flow. Phase acceptance remains open.
- **Phase 4:** the engineering image now includes Firefox and upstream network,
  audio, Bluetooth, display, lock, and brightness tools with Waybar/hardware
  entry points, and sets Firefox as the system XDG browser default. Settings
  now shows the active Linux user/session, lock and sign-out actions, installed
  system locales and time zones, automatic time-sync controls, and upstream
  device-control links. The image starts a graphical Polkit agent and ships an
  English locale. Evermore translation, additional-account administration,
  graphical power policy, onboarding, and complete daily-use recovery remain
  open.
- **Phase 5:** searchable in-panel troubleshooting and an opt-in sanitized
  diagnostic export are implemented. Settings also reports per-identity
  Blossom sync service health with recent-log access, setup guidance, and local
  sync-folder file count and size. File-level outcomes and conflict recovery
  remain open, along with accessibility review and localization preparation.
- **Phase 6:** acceptance remains open; no readiness gate has been verified.

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
status with deliberate device revocation. Expose public NIP-01 profile
metadata editing separately from encrypted application preferences. Keep
per-device network choices local.

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

### System settings extension

**Deliver:** a System section for the current Linux user/session, lock and
sign-out, system locale, regional formats through the OS locale, time-zone
selection, automatic time-sync status, and upstream network/audio/display/
Bluetooth entry points. Persist locale, time-zone, and automatic time-sync
preferences through the identity-encrypted settings domain on the local
control-plane relay; apply them to each device through its OS after
confirmation and authorization.
Keep Linux accounts/session operations and hardware state local. Keep Linux
accounts distinct from Evermore identities; first-user enrollment remains in
the greeter.

**Gate:** only installed locale/time-zone values can be applied; relay and OS
results are reported separately; authorization cancellation leaves the local
OS value unchanged even if the identity preference was already saved; locale
changes state their system-wide scope and sign-out requirement; time-zone
changes update the quiet-hours clock. Evermore remains labeled English-only
until translated strings ship.

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
- Locale, time-zone, and automatic time-sync preferences sync by identity;
  applying them changes the selected device's OS setting. Session actions and
  hardware state remain local.
- The local control-plane relay carries identity-encrypted settings; machine
  administration still uses OS services and authorization.
- English is the initial language. Translation follows localization
  preparation.
- Use upstream desktop applications for browser, files, terminal, and hardware
  controls. Evermore owns its identity, trust, approvals, AI context, settings,
  notifications, and service status.

